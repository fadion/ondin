//! The Chart card (`charts.md`, *The Chart card*): a chart frame's description,
//! edited section by section. Every edit is one `SetChart` of the whole
//! description; a scrubbed field previews through `EditorSession::set_preview`,
//! which draws the parts the edit would generate.

use super::paint::{ChartColour, PaintSlot};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, Prefix, Scrub};
use eframe::egui;
use ondin_core::chart::{
    self, Align, AxisSide, Chart, ChartKind, Curve, FlowAlign, FlowColour, LabelAngle,
    LegendPosition, NodeLabels, Palette, PieLabel, PointShape, PointStyle, Preset, Stacking,
    ValuePosition,
};
use ondin_core::kurbo::{self, Affine, BezPath, Shape};
use ondin_core::{Brush, NodeId, Operation, Transaction};
use std::hash::{Hash, Hasher};

/// The sections a card opens folded: the options a new chart rarely needs first.
pub(crate) const FOLDED: [&str; 5] = [
    "Chart marks",
    "Chart axes",
    "Chart values",
    "Chart legend",
    "Chart text",
];

/// A Style cell's height: the picture and its name under it.
/// Past two control heights, so it reads (and audits) as a tile, not a control.
const STYLE_CELL_H: f32 = 58.0;
/// The picture inside a Style cell, narrow enough for five cells to a row.
const STYLE_PICTURE: egui::Vec2 = egui::vec2(44.0, 28.0);
/// The number the *Format* dropdown writes as its sample.
const FORMAT_SAMPLE: f64 = 12_345.6;
/// The *Format* dropdown's popup width: two text fields to a row.
const FORMAT_POPUP_W: f32 = 220.0;
/// The series style menu's width, past its longest row.
const SERIES_MENU_W: f32 = 140.0;
/// The size a Style cell's chart is generated at before it is drawn small, so
/// its marks keep a chart's proportions rather than a thumbnail's.
const STYLE_CHART: kurbo::Size = kurbo::Size::new(320.0, 200.0);
/// What the card measured of its chart, kept while the description stays the same:
/// the Style cells' pictures and the generator's left-out values.
pub(crate) struct ChartCardCache {
    id: NodeId,
    key: u64,
    thumbs: Vec<(Preset, egui::TextureHandle)>,
    left_out: Vec<String>,
    refusal: Option<String>,
    /// Whether the chart draws a progress ring's track, which the Marks section
    /// then offers a colour for.
    track: bool,
}

/// The kinds' variants as the Type section's segments name them, with the
/// variant `chart` is at; `None` for a kind with none to pick.
fn variants(chart: &Chart) -> Option<(&'static [&'static str], usize)> {
    let o = &chart.options;
    let stacking = |s: Stacking| match s {
        Stacking::Apart => 0,
        Stacking::Stacked => 1,
        Stacking::Percent => 2,
    };
    Some(match chart.kind {
        ChartKind::Column | ChartKind::Bar => {
            (&["Grouped", "Stacked", "100%"], stacking(o.bars.stacking))
        }
        ChartKind::Line => (
            &["Straight", "Smooth", "Step"],
            match o.lines.curve {
                Curve::Straight => 0,
                Curve::Smooth => 1,
                Curve::Step => 2,
            },
        ),
        ChartKind::Area => (
            &["Overlapping", "Stacked", "100%"],
            stacking(o.lines.area_stacking),
        ),
        ChartKind::Pie => (&["Pie", "Donut"], usize::from(o.slices.donut)),
        ChartKind::Scatter | ChartKind::Sankey => return None,
    })
}

/// `chart` at variant `i` of its kind ([`variants`]).
fn with_variant(chart: &Chart, i: usize) -> Chart {
    let mut next = chart.clone();
    let stacking = [Stacking::Apart, Stacking::Stacked, Stacking::Percent];
    let o = &mut next.options;
    match chart.kind {
        ChartKind::Column | ChartKind::Bar => o.bars.stacking = stacking[i.min(2)],
        ChartKind::Line => o.lines.curve = [Curve::Straight, Curve::Smooth, Curve::Step][i.min(2)],
        ChartKind::Area => o.lines.area_stacking = stacking[i.min(2)],
        ChartKind::Pie => o.slices.donut = i == 1,
        ChartKind::Scatter | ChartKind::Sankey => {}
    }
    next
}

/// The paint a chart slot holds, bound or not.
pub(crate) fn fill_of(chart: &Chart, colour: ChartColour) -> Option<ondin_core::BrushSource> {
    match colour {
        ChartColour::Series(col) => chart::series_fill(chart, col),
        ChartColour::Node(i) => Some(chart::sankey_nodes(chart).into_iter().nth(i)?.1),
        ChartColour::Ink => Some(chart.options.text.ink.clone()),
        ChartColour::Axis(value) => Some(axis_of(chart, value).colour.clone()),
        ChartColour::Grid(value) => Some(axis_of(chart, value).grid.colour.clone()),
        ChartColour::Neutral => Some(chart.options.value_axis.colour.clone()),
        ChartColour::Track => Some(chart.options.value_axis.grid.colour.clone()),
    }
}

/// The value axis when `value`, else the category axis.
fn axis_of(chart: &Chart, value: bool) -> &chart::Axis {
    if value {
        &chart.options.value_axis
    } else {
        &chart.options.category_axis
    }
}

/// The colour a chart slot draws in on chart `id`, solid, a binding read in
/// the mode in force there.
pub(crate) fn colour_of(
    doc: &ondin_core::Document,
    id: NodeId,
    chart: &Chart,
    colour: ChartColour,
) -> Option<ondin_core::peniko::Color> {
    let fill = fill_of(chart, colour)?;
    match doc.resolve_brush(id, &fill).into_owned() {
        Brush::Solid(c) => Some(c),
        _ => None,
    }
}

/// Series `col`'s colour on chart `id`, solid: the data window's header swatch.
pub(crate) fn series_colour(
    doc: &ondin_core::Document,
    id: NodeId,
    chart: &Chart,
    col: chart::ColumnId,
) -> Option<ondin_core::peniko::Color> {
    colour_of(doc, id, chart, ChartColour::Series(col))
}

/// `chart` with the slot's colour set to `c`.
pub(crate) fn with_colour(
    chart: &Chart,
    colour: ChartColour,
    c: ondin_core::peniko::Color,
) -> Chart {
    with_fill(
        chart,
        colour,
        ondin_core::BrushSource::Literal(Brush::Solid(c)),
    )
}

/// `chart` with the slot's paint set to `fill`, bound or not.
pub(crate) fn with_fill(
    chart: &Chart,
    colour: ChartColour,
    fill: ondin_core::BrushSource,
) -> Chart {
    match colour {
        ChartColour::Series(col) => chart::with_series_fill(chart, col, fill),
        ChartColour::Node(i) => match chart::sankey_nodes(chart).into_iter().nth(i) {
            Some((name, _)) => chart::with_node_fill(chart, &name, fill),
            None => chart.clone(),
        },
        ChartColour::Ink
        | ChartColour::Axis(_)
        | ChartColour::Grid(_)
        | ChartColour::Neutral
        | ChartColour::Track => {
            let mut next = chart.clone();
            let o = &mut next.options;
            *match colour {
                ChartColour::Axis(true) | ChartColour::Neutral => &mut o.value_axis.colour,
                ChartColour::Axis(false) => &mut o.category_axis.colour,
                ChartColour::Grid(true) | ChartColour::Track => &mut o.value_axis.grid.colour,
                ChartColour::Grid(false) => &mut o.category_axis.grid.colour,
                _ => &mut o.text.ink,
            } = fill;
            next
        }
    }
}

/// What the cache is keyed by: everything the generator reads but the parts.
fn description_key(id: NodeId, chart: &Chart, size: kurbo::Size) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    id.hash(&mut h);
    format!("{:?}", chart.kind).hash(&mut h);
    format!("{:?}", chart.mapping).hash(&mut h);
    format!("{:?}", chart.series).hash(&mut h);
    format!("{:?}", chart.nodes).hash(&mut h);
    format!("{:?}", chart.options).hash(&mut h);
    chart.table.rows.hash(&mut h);
    for c in &chart.table.columns {
        c.name.hash(&mut h);
    }
    size.width.to_bits().hash(&mut h);
    size.height.to_bits().hash(&mut h);
    h.finish()
}

/// A Style cell's picture: `chart` generated at [`STYLE_CHART`] and its fills and
/// strokes rasterised at `px`, text left out, since no label reads at this size.
fn style_picture(
    ctx: &egui::Context,
    chart: &Chart,
    px: [usize; 2],
) -> Option<egui::TextureHandle> {
    let family = ondin_core::typography::TextStyle::default().font_family;
    let generated = chart::generate(chart, STYLE_CHART, &family).ok()?;
    let k = (px[0] as f64 / STYLE_CHART.width).min(px[1] as f64 / STYLE_CHART.height);
    let mut world: std::collections::HashMap<chart::PartKey, Affine> = Default::default();
    let mut art: Vec<(BezPath, ondin_core::peniko::Color)> = Vec::new();
    // A gradient flow draws in its source's colour here: one colour reads at this size.
    let solid = |b: &ondin_core::BrushSource, alpha: f32| match b.literal() {
        Some(brush @ (Brush::Solid(_) | Brush::Gradient(_))) => super::paint::stops_of(brush)
            .first()
            .map(|(_, c)| c.multiply_alpha(alpha)),
        _ => None,
    };
    for p in &generated.parts {
        let parent: Affine = p
            .parent
            .and_then(|k| world.get(&k).copied())
            .unwrap_or_default();
        let at = parent * p.transform;
        world.insert(p.key, at);
        let Some(outline) = ondin_core::geometry::local_path(&p.kind) else {
            continue;
        };
        let to_px = Affine::scale(k) * at;
        for f in p.fills.iter().filter(|f| f.visible) {
            if let Some(c) = solid(&f.brush, p.opacity) {
                art.push((to_px * outline.clone(), c));
            }
        }
        for s in p.strokes.iter().filter(|s| s.visible) {
            if let Some(c) = solid(&s.brush, p.opacity) {
                let pen = kurbo::Stroke::new(s.width)
                    .with_join(s.join)
                    .with_caps(s.cap);
                let ink = kurbo::stroke(
                    outline.path_elements(0.1),
                    &pen,
                    &kurbo::StrokeOpts::default(),
                    0.1,
                );
                art.push((to_px * ink, c));
            }
        }
    }
    let rgba = ondin_render::rasterize_paths(px[0] as u32, px[1] as u32, &art)?;
    let image = egui::ColorImage::from_rgba_unmultiplied(px, &rgba);
    Some(ctx.load_texture("chart-style", image, egui::TextureOptions::LINEAR))
}

impl OndinApp {
    /// Chart `id`'s slot `colour` as bound: its variable and `%`; `None` when
    /// it holds a colour of its own.
    pub(crate) fn chart_binding(
        &self,
        id: NodeId,
        colour: ChartColour,
    ) -> Option<(ondin_core::VarId, f32)> {
        match fill_of(self.session.chart_drawn(id)?, colour)? {
            ondin_core::BrushSource::Bound { var, alpha } => Some((var, alpha)),
            ondin_core::BrushSource::Literal(_) => None,
        }
    }

    /// The `SetChart` binding chart `id`'s slot `colour` to `var` at `alpha`
    /// (its `%` kept when `None`), or for `var` `None` detaching it to the
    /// colour it draws.
    pub(crate) fn chart_bind_tx(
        &self,
        id: NodeId,
        colour: ChartColour,
        var: Option<ondin_core::VarId>,
        alpha: Option<f32>,
    ) -> Transaction {
        let doc = &self.session.doc;
        let Some(chart) = doc.get(id).and_then(|n| n.chart()) else {
            return Transaction::default();
        };
        let Some(now) = fill_of(chart, colour) else {
            return Transaction::default();
        };
        let fill = match var {
            Some(var) => ondin_core::BrushSource::Bound {
                var,
                alpha: alpha.unwrap_or(match &now {
                    ondin_core::BrushSource::Bound { alpha, .. } => *alpha,
                    ondin_core::BrushSource::Literal(Brush::Solid(c)) => c.components[3],
                    ondin_core::BrushSource::Literal(_) => 1.0,
                }),
            },
            None if now.is_bound() => doc.resolve_brush(id, &now).into_owned().into(),
            None => return Transaction::default(),
        };
        Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(with_fill(chart, colour, fill))),
        }])
    }

    /// The Chart card, for one chart frame.
    pub(super) fn inspector_chart(&mut self, ui: &mut egui::Ui, id: NodeId) {
        self.inspector_charts(ui, &[id]);
    }

    /// The charts of a multi-selection, when every selected layer is a chart and
    /// all are one kind; `None` otherwise.
    pub(super) fn selected_charts(&self) -> Option<Vec<NodeId>> {
        let ids = self.session.selection.ids();
        let doc = &self.session.doc;
        let kind = doc.get(*ids.first()?)?.chart()?.kind;
        ids.iter()
            .all(|id| {
                doc.get(*id)
                    .and_then(|n| n.chart())
                    .is_some_and(|c| c.kind == kind)
            })
            .then(|| ids.to_vec())
    }

    /// The Chart card over `ids`, the first the one it shows: every edit writes
    /// each ([`Self::chart_tx`]). Several charts leave out the Data and Series
    /// sections, which belong to one chart's table.
    pub(super) fn inspector_charts(&mut self, ui: &mut egui::Ui, ids: &[NodeId]) {
        let Some(&id) = ids.first() else {
            return;
        };
        let Some(chart) = self.session.chart_drawn(id).cloned() else {
            return;
        };
        self.chart_others = ids[1..].to_vec();
        self.refresh_chart_cache(ui.ctx(), id, &chart);
        let several = ids.len() > 1;
        self.panel(ui, "Chart", None, |app, ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            app.chart_section(ui, "Chart type", "Type", |app, ui| {
                app.chart_type_rows(ui, id, &chart);
            });
            app.chart_section(ui, "Chart style", "Style", |app, ui| {
                app.chart_style_rows(ui, id, &chart);
            });
            app.chart_section(ui, "Chart data", "Data", |app, ui| {
                if several {
                    ui.label(
                        egui::RichText::new("Edit data one chart at a time")
                            .size(ui::SMALL_PT)
                            .color(theme::text::muted()),
                    );
                } else {
                    app.chart_data_rows(ui, id, &chart);
                }
            });
            if !several {
                let series = if chart.kind == ChartKind::Sankey {
                    "Nodes"
                } else {
                    "Series"
                };
                app.chart_section(ui, "Chart series", series, |app, ui| {
                    app.chart_series_rows(ui, id, &chart);
                });
            }
            app.chart_section(ui, "Chart marks", "Marks", |app, ui| {
                app.chart_mark_rows(ui, id, &chart);
            });
            if chart.kind.has_axes() {
                app.chart_section(ui, "Chart axes", "Axes", |app, ui| {
                    app.chart_axis_rows(ui, id, &chart);
                });
            }
            if chart.kind != ChartKind::Sankey {
                app.chart_section(ui, "Chart values", "Values", |app, ui| {
                    app.chart_value_rows(ui, id, &chart);
                });
                app.chart_section(ui, "Chart legend", "Legend", |app, ui| {
                    app.chart_legend_rows(ui, id, &chart);
                });
            }
            app.chart_section(ui, "Chart text", "Text", |app, ui| {
                app.chart_text_rows(ui, id, &chart);
            });
            let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
            let tip = crate::input::keys("Keep the shapes and drop the chart's options (Ctrl+E)");
            if ui::action_button(
                ui,
                icon::STACK_SIMPLE,
                "Flatten chart",
                ui::FieldButton::Off,
                size,
            )
            .on_hover_text(tip)
            .clicked()
            {
                app.flatten_charts(ids);
            }
        });
        self.chart_others.clear();
    }

    /// Rebuild [`ChartCardCache`] when the description or the drawn size moved.
    fn refresh_chart_cache(&mut self, ctx: &egui::Context, id: NodeId, chart: &Chart) {
        let size = match self.session.resolved.used_kind(&self.session.doc, id) {
            Some(ondin_core::NodeKind::Artboard { size, .. }) => *size,
            _ => return,
        };
        // Drawn in the colours its bindings read now, so a mode switch redraws it.
        let mut drawn = chart.clone();
        for b in drawn.brushes_mut().filter(|b| b.is_bound()) {
            *b = self.session.doc.resolve_brush(id, b).into_owned().into();
        }
        let chart = &drawn;
        let key = description_key(id, chart, size);
        if self
            .chart_card
            .as_ref()
            .is_some_and(|c| c.id == id && c.key == key)
        {
            return;
        }
        let family = ondin_core::typography::TextStyle::default().font_family;
        let (left_out, refusal, track) = match chart::generate(chart, size, &family) {
            Ok(g) => {
                let track =
                    chart.kind == ChartKind::Pie && g.parts.iter().any(|p| p.name == "Track");
                (g.left_out, None, track)
            }
            Err(chart::Refusal::TooManyMarks { count, what }) => (
                Vec::new(),
                Some(format!(
                    "Too many marks: {} {what}, at most {}",
                    chart::grouped(count),
                    chart::grouped(chart::MAX_MARKS)
                )),
                false,
            ),
            Err(chart::Refusal::LoopsBack(path)) => {
                (Vec::new(), Some(format!("Flows loop back: {path}")), false)
            }
        };
        let ppp = ctx.pixels_per_point();
        let px = [
            (STYLE_PICTURE.x * ppp).round() as usize,
            (STYLE_PICTURE.y * ppp).round() as usize,
        ];
        let thumbs = Preset::ALL
            .into_iter()
            .filter_map(|p| {
                let styled = Chart {
                    options: p.apply(&chart.options),
                    ..chart.clone()
                };
                style_picture(ctx, &styled, px).map(|t| (p, t))
            })
            .collect();
        self.chart_card = Some(ChartCardCache {
            id,
            key,
            thumbs,
            left_out,
            refusal,
            track,
        });
    }

    /// One of the card's sections: a fold head and, open, its rows. `key` is the
    /// fold's memory in `collapsed_panels`.
    fn chart_section(
        &mut self,
        ui: &mut egui::Ui,
        key: &'static str,
        label: &str,
        body: impl FnOnce(&mut Self, &mut egui::Ui),
    ) {
        let open = !self.collapsed_panels.contains(key);
        if ui::fold_head(ui, label, open).0.clicked() {
            if open {
                self.collapsed_panels.insert(key);
            } else {
                self.collapsed_panels.remove(key);
            }
        }
        if open {
            body(self, ui);
        }
    }

    /// Write `next` over chart `id`'s description as one undo step named `label`,
    /// unless it is what the chart already holds.
    fn chart_commit(&mut self, id: NodeId, label: &'static str, next: Chart) {
        if self.session.doc.get(id).and_then(|n| n.chart()) == Some(&next) {
            return;
        }
        self.session.name_step(label, icon::CHART);
        let tx = self.chart_tx(id, next);
        self.commit_edit(tx);
    }

    /// `next` written over chart `id`, and the change carried to every other
    /// chart the card writes ([`OndinApp::chart_others`]): the kind, and each
    /// option that changed, each chart keeping its own data and the rest.
    fn chart_tx(&self, id: NodeId, next: Chart) -> Transaction {
        let doc = &self.session.doc;
        let mut ops = Vec::new();
        if let Some(before) = doc.get(id).and_then(|n| n.chart()) {
            for other in &self.chart_others {
                let Some(own) = doc.get(*other).and_then(|n| n.chart()) else {
                    continue;
                };
                let mut carried = own.clone();
                if next.kind != before.kind {
                    carried.kind = next.kind;
                    if next.kind == ChartKind::Sankey || own.kind == ChartKind::Sankey {
                        carried.mapping = chart::Mapping::guess(next.kind, &own.table);
                    }
                }
                carried.options =
                    chart::Options::carried(&before.options, &next.options, &own.options);
                if carried != *own {
                    ops.push(Operation::SetChart {
                        id: *other,
                        chart: Some(Box::new(carried)),
                    });
                }
            }
        }
        ops.insert(
            0,
            Operation::SetChart {
                id,
                chart: Some(Box::new(next)),
            },
        );
        Transaction(ops)
    }

    /// A number field over one option: scrubbed, it previews; released or
    /// typed, it commits as `label`.
    #[allow(clippy::too_many_arguments)]
    fn chart_number(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        width: f32,
        prefix: &'static str,
        label: &'static str,
        scrub: Scrub,
        decimals: usize,
        get: impl Fn(&Chart) -> f64,
        set: impl Fn(&mut Chart, f64),
    ) {
        let mut v = get(chart);
        let resp = ui::value_field(
            ui,
            egui::vec2(width, ui::CONTROL_H),
            Prefix::Label(prefix),
            &mut v,
            scrub,
            |d| d.custom_formatter(ui::number(decimals)),
        )
        .on_hover_text(label);
        let mut next = chart.clone();
        set(&mut next, v);
        let tx = self.chart_tx(id, next);
        self.named_valve(label, icon::CHART, &resp, tx);
    }

    /// Two [`Self::chart_number`]s on one line.
    fn chart_pair(&mut self, ui: &mut egui::Ui, add: impl FnOnce(&mut Self, &mut egui::Ui, f32)) {
        let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            add(self, ui, half);
        });
    }

    /// A choice of a few, as segments: picking one commits `set` as `label`.
    #[allow(clippy::too_many_arguments)]
    fn chart_segments(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        names: &[&str],
        at: usize,
        set: impl Fn(&mut Chart, usize),
    ) {
        let pick = ui::segmented_tipped(
            ui,
            ui.available_width(),
            ui::SEGMENT_CELL_H,
            names.len(),
            at,
            |_| true,
            |_| label,
            |p, i, r, on| ui::segment_label(p, r, names[i], on),
        );
        if let Some(i) = pick.filter(|i| *i != at) {
            let mut next = chart.clone();
            set(&mut next, i);
            self.chart_commit(id, label, next);
        }
    }

    /// A switch over one option, committing `set` as `label`.
    fn chart_switch(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        label: &'static str,
        on: bool,
        set: impl Fn(&mut Chart, bool),
    ) {
        if ui::switch_row(ui, label, on, ui::CONTROL_H).clicked() {
            let mut next = chart.clone();
            set(&mut next, !on);
            self.chart_commit(id, label, next);
        }
    }

    /// A dropdown over one option: its rows `names`, `at` lit.
    #[allow(clippy::too_many_arguments)]
    fn chart_menu(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: &'static str,
        label: &'static str,
        names: &[&str],
        at: usize,
        set: impl Fn(&mut Chart, usize),
    ) {
        let mut pick = None;
        ui::labelled(ui, label, |ui| {
            egui::ComboBox::from_id_salt(salt)
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::combo_text(names.get(at).copied().unwrap_or("")))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(i == at, *name).clicked() {
                            pick = Some(i);
                        }
                    }
                });
        });
        if let Some(i) = pick.filter(|i| *i != at) {
            let mut next = chart.clone();
            set(&mut next, i);
            self.chart_commit(id, label, next);
        }
    }

    fn chart_type_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let mut pick = None;
        egui::ComboBox::from_id_salt("chart-kind")
            .icon(ui::combo_chevron)
            .width(ui.available_width())
            .selected_text(ui::glyph_and_text(
                crate::rail::chart_glyph(chart.kind),
                chart.kind.label(),
            ))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                for kind in ChartKind::ALL {
                    let row = ui::glyph_and_text(crate::rail::chart_glyph(kind), kind.label());
                    if ui.selectable_label(kind == chart.kind, row).clicked() {
                        pick = Some(kind);
                    }
                }
            });
        if let Some(kind) = pick.filter(|k| *k != chart.kind) {
            let mut next = chart.clone();
            next.kind = kind;
            if kind == ChartKind::Sankey || chart.kind == ChartKind::Sankey {
                next.mapping = chart::Mapping::guess(kind, &chart.table);
            }
            self.chart_commit(id, "Chart type", next);
        }
        if let Some((names, at)) = variants(chart) {
            self.chart_segments(ui, id, chart, "Chart type", names, at, |c, i| {
                *c = with_variant(c, i);
            });
        }
    }

    fn chart_style_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let lit = Preset::of(&chart.options);
        let thumbs: Vec<(Preset, egui::TextureId)> = self
            .chart_card
            .as_ref()
            .filter(|c| c.id == id)
            .map(|c| c.thumbs.iter().map(|(p, t)| (*p, t.id())).collect())
            .unwrap_or_default();
        let n = Preset::ALL.len() as f32;
        let cell = (ui.available_width() - ui::CARD_COL_GAP * (n - 1.0)) / n;
        let mut pick = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            for preset in Preset::ALL {
                let picture = thumbs.iter().find(|(p, _)| *p == preset).map(|(_, t)| *t);
                if style_cell(ui, cell, preset.label(), picture, lit == Some(preset)).clicked() {
                    pick = Some(preset);
                }
            }
        });
        if let Some(p) = pick {
            let next = Chart {
                options: p.apply(&chart.options),
                ..chart.clone()
            };
            self.chart_commit(id, "Chart style", next);
        }
    }

    fn chart_data_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let rows = chart.table.rows.len();
        let summary = match chart.kind {
            ChartKind::Sankey => ui::count_as(rows, "flow", "flows"),
            _ => format!(
                "{} · {}",
                ui::count_as(chart.mapping.series.len(), "series", "series"),
                ui::count_as(rows, "row", "rows")
            ),
        };
        ui.label(
            egui::RichText::new(summary)
                .size(ui::SMALL_PT)
                .color(theme::text::muted()),
        );
        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
        if ui::action_button(ui, icon::TABLE, "Edit data…", ui::FieldButton::Off, size)
            .on_hover_text("Open the chart's table under the canvas")
            .clicked()
        {
            self.open_chart_data(id, (0, 0));
        }
        self.chart_source_line(ui, id);
        if let Some(cache) = self.chart_card.as_ref().filter(|c| c.id == id) {
            if let Some(why) = &cache.refusal {
                ui.label(
                    egui::RichText::new(why)
                        .size(ui::SMALL_PT)
                        .color(color::warn()),
                );
            } else if !cache.left_out.is_empty() {
                let n = cache.left_out.len();
                ui.label(
                    egui::RichText::new(format!("{} left out", ui::count_as(n, "value", "values")))
                        .size(ui::SMALL_PT)
                        .color(color::warn()),
                )
                .on_hover_text(cache.left_out.join("\n"));
            }
        }
        let names: Vec<&str> = chart
            .table
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        let ids: Vec<chart::ColumnId> = chart.table.columns.iter().map(|c| c.id).collect();
        let at = |c: Option<chart::ColumnId>| c.and_then(|c| ids.iter().position(|i| *i == c));
        let m = &chart.mapping;
        match chart.kind {
            ChartKind::Sankey => {
                for (salt, label, now, which) in [
                    ("chart-from", "From", m.from, 0),
                    ("chart-to", "To", m.to, 1),
                    ("chart-value", "Value", m.value, 2),
                ] {
                    let ids = ids.clone();
                    self.chart_menu(
                        ui,
                        id,
                        chart,
                        salt,
                        label,
                        &names,
                        at(now).unwrap_or(usize::MAX),
                        move |c, i| {
                            let col = Some(ids[i]);
                            match which {
                                0 => c.mapping.from = col,
                                1 => c.mapping.to = col,
                                _ => c.mapping.value = col,
                            }
                        },
                    );
                }
            }
            kind => {
                let label = if kind == ChartKind::Scatter {
                    "X"
                } else {
                    "Category"
                };
                let ids2 = ids.clone();
                self.chart_menu(
                    ui,
                    id,
                    chart,
                    "chart-category",
                    label,
                    &names,
                    at(m.category).unwrap_or(usize::MAX),
                    move |c, i| {
                        c.mapping.category = Some(ids2[i]);
                        c.mapping.series.retain(|s| *s != ids2[i]);
                    },
                );
            }
        }
    }

    fn chart_series_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let palettes = [
            (Palette::Ondin, "Ondin"),
            (Palette::Cool, "Cool"),
            (Palette::Warm, "Warm"),
            (Palette::Mono, "Mono"),
        ];
        let groups = colour_groups(&self.session.doc);
        let bound = chart::bound_variables(chart);
        // A group lights while the series run through it in order; a palette
        // while nothing is bound.
        let lit_group = bound.as_ref().and_then(|b| {
            groups.iter().position(|(_, vars)| {
                b.iter()
                    .enumerate()
                    .all(|(i, v)| vars[i % vars.len()] == *v)
            })
        });
        let shown = match (lit_group, &bound) {
            (Some(g), _) => format!("From variables · {}", groups[g].0),
            (None, Some(_)) => "From variables".to_string(),
            (None, None) => palettes
                .iter()
                .find(|(p, _)| *p == chart.options.palette)
                .map_or("Ondin", |(_, n)| *n)
                .to_string(),
        };
        enum Pick {
            Palette(Palette),
            Group(usize),
        }
        let mut pick = None;
        ui::labelled(ui, "Palette", |ui| {
            egui::ComboBox::from_id_salt(("chart-palette", id))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                .selected_text(ui::combo_text(&shown))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    for (p, name) in palettes {
                        let on = bound.is_none() && p == chart.options.palette;
                        if ui.selectable_label(on, name).clicked() {
                            pick = Some(Pick::Palette(p));
                        }
                    }
                    ui::menu_sep(ui);
                    ui.label(ui::eyebrow("From variables"));
                    if groups.is_empty() {
                        ui.label(
                            egui::RichText::new("No colour variables yet")
                                .size(ui::SMALL_PT)
                                .color(theme::text::muted()),
                        );
                    }
                    for (g, (name, _)) in groups.iter().enumerate() {
                        if ui.selectable_label(lit_group == Some(g), name).clicked() {
                            pick = Some(Pick::Group(g));
                        }
                    }
                });
        });
        match pick {
            Some(Pick::Palette(p)) => {
                self.chart_commit(id, "Palette", chart::with_palette(chart, p));
            }
            Some(Pick::Group(g)) if lit_group != Some(g) => {
                let next = chart::with_variables(chart, &groups[g].1);
                self.chart_commit(id, "Palette", next);
            }
            _ => {}
        }
        let totals = chart::sankey_totals(chart);
        let rows: Vec<(ChartColour, String, Option<bool>)> = match chart.kind {
            ChartKind::Sankey => chart::sankey_nodes(chart)
                .into_iter()
                .enumerate()
                .map(|(i, (name, _))| (ChartColour::Node(i), name, None))
                .collect(),
            _ => chart
                .table
                .columns
                .iter()
                .filter(|c| Some(c.id) != chart.mapping.category)
                .map(|c| {
                    let mapped = chart.mapping.series.contains(&c.id);
                    let visible = chart
                        .series
                        .iter()
                        .find(|s| s.column == c.id)
                        .is_none_or(|s| s.visible);
                    (
                        ChartColour::Series(c.id),
                        c.name.clone(),
                        Some(mapped && visible),
                    )
                })
                .collect(),
        };
        let w = ui.available_width();
        let styled = matches!(
            chart.kind,
            ChartKind::Line | ChartKind::Area | ChartKind::Scatter
        );
        for (slot, name, shown) in rows {
            let colour = colour_of(&self.session.doc, id, chart, slot).map(ui::color_to_egui);
            let total = match slot {
                ChartColour::Node(i) => totals.get(i).map(|v| {
                    chart::scale::format(
                        *v,
                        &chart.options.values.format,
                        chart::scale::decimals_of(*v).min(3),
                    )
                }),
                _ => None,
            };
            let style_button = styled && matches!(slot, ChartColour::Series(_));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let buttons = usize::from(shown.is_some()) + usize::from(style_button);
                let field = w - buttons as f32 * (ui::CONTROL_H + ui::CARD_COL_GAP);
                let swatch = match (colour, shown) {
                    (_, Some(false)) | (None, _) => ui::Swatch::Mixed,
                    (Some(c), _) => ui::Swatch::Solid(c),
                };
                self.chart_colour_row(
                    ui,
                    id,
                    slot,
                    field,
                    swatch,
                    &name,
                    shown != Some(false),
                    total.as_deref(),
                );
                if let (true, ChartColour::Series(col)) = (style_button, slot) {
                    self.chart_series_style(ui, id, chart, col);
                }
                if let (Some(on), ChartColour::Series(col)) = (shown, slot) {
                    let (glyph, tip) = if on {
                        (icon::EYE, "Hide this series")
                    } else {
                        (icon::EYE_SLASH, "Show this column as a series")
                    };
                    if ui::field_button(ui, glyph, ui::CONTROL_H, ui::FieldButton::Off)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        let mut next = chart.clone();
                        show_series(&mut next, col, !on);
                        self.chart_commit(id, "Show series", next);
                    }
                }
            });
        }
    }

    /// One of the chart's own colours (not a series') as a full-width
    /// [`Self::chart_colour_row`] named `name`.
    fn chart_slot_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        slot: ChartColour,
        name: &str,
    ) {
        let swatch = colour_of(&self.session.doc, id, chart, slot).map_or(ui::Swatch::Mixed, |c| {
            ui::Swatch::Solid(ui::color_to_egui(c))
        });
        let width = ui.available_width();
        self.chart_colour_row(ui, id, slot, width, swatch, name, true, None);
    }

    /// One colour of the chart as a field `width` wide: its swatch, which opens
    /// the picker on the slot while `live`, its name (dim when not), a
    /// right-aligned `total`, and, bound, its variable's name in a capsule.
    #[allow(clippy::too_many_arguments)]
    fn chart_colour_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        slot: ChartColour,
        width: f32,
        swatch: ui::Swatch,
        name: &str,
        live: bool,
        total: Option<&str>,
    ) {
        ui::field_row(ui, egui::vec2(width, ui::CONTROL_H), |ui| {
            ui.spacing_mut().item_spacing.x = 7.0;
            let resp = ui::swatch(ui, 14.0, theme::color::field(), swatch);
            if resp.clicked() && live {
                self.open_picker(ui, id, PaintSlot::Chart(slot), resp.rect.left_top());
            }
            let ink = if live {
                color::text()
            } else {
                theme::text::off()
            };
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(total) = total {
                    ui.label(
                        egui::RichText::new(total)
                            .size(ui::CONTROL_PT)
                            .color(theme::text::muted()),
                    )
                    .on_hover_text("The node's total: the larger of what flows in and out");
                }
                if let Some((var, _)) = self.chart_binding(id, slot) {
                    let doc = &self.session.doc;
                    let var_name = doc.variables().get(var).map_or("", |v| v.name.as_str());
                    let max_w = (ui.available_width() / 2.0).max(0.0);
                    let w = super::binding::capsule_width(ui, var_name, max_w);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(w, ui.available_height()),
                        egui::Sense::hover(),
                    );
                    super::binding::capsule(ui, rect.left_center(), var_name, max_w);
                    resp.on_hover_text(super::binding::chip_tip(doc, id, var));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(name).size(ui::CONTROL_PT).color(ink))
                            .truncate(),
                    );
                });
            });
        });
    }

    /// A series row's style button: a menu of its line's dash (a line or area)
    /// and its points' shape, each pick one step.
    fn chart_series_style(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        col: chart::ColumnId,
    ) {
        let style = chart.series.iter().find(|s| s.column == col);
        let dash = style.map_or(chart::Dash::Solid, |s| s.dash);
        let point = style.and_then(|s| s.point);
        let lined = chart.kind != ChartKind::Scatter;
        let tip = if lined {
            "Line and point style"
        } else {
            "Point shape"
        };
        let button = ui::field_button(ui, icon::DOTS_THREE, ui::CONTROL_H, ui::FieldButton::Off)
            .on_hover_text(tip);
        // The dash, or the point shape (`Err`) picked.
        let mut pick: Option<Result<chart::Dash, Option<PointShape>>> = None;
        egui::Popup::menu(&button)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::popup_rows(ui);
                ui.set_min_width(SERIES_MENU_W);
                if lined {
                    ui.label(ui::eyebrow("Line"));
                    for (d, name) in [
                        (chart::Dash::Solid, "Solid"),
                        (chart::Dash::Dashed, "Dashed"),
                        (chart::Dash::Dotted, "Dotted"),
                    ] {
                        if ui.selectable_label(d == dash, name).clicked() && d != dash {
                            pick = Some(Ok(d));
                        }
                    }
                    ui.label(ui::eyebrow("Points"));
                }
                // A line's unshaped points are circles, so it lists Circle once;
                // a scatter's take the chart's shape.
                let shapes: &[(Option<PointShape>, &str)] = if lined {
                    &[
                        (None, "Circle"),
                        (Some(PointShape::Square), "Square"),
                        (Some(PointShape::Diamond), "Diamond"),
                        (Some(PointShape::Triangle), "Triangle"),
                    ]
                } else {
                    &[
                        (None, "Chart's shape"),
                        (Some(PointShape::Circle), "Circle"),
                        (Some(PointShape::Square), "Square"),
                        (Some(PointShape::Diamond), "Diamond"),
                        (Some(PointShape::Triangle), "Triangle"),
                    ]
                };
                let point = match point {
                    Some(PointShape::Circle) if lined => None,
                    p => p,
                };
                for (p, name) in shapes {
                    let on = *p == point;
                    if ui.selectable_label(on, *name).clicked() && !on {
                        pick = Some(Err(*p));
                    }
                }
            });
        if let Some(pick) = pick {
            let mut next = chart.clone();
            if !next.series.iter().any(|s| s.column == col)
                && let Some(fill) = chart::series_fill(chart, col)
            {
                next = chart::with_series_fill(&next, col, fill);
            }
            let s = next.series.iter_mut().find(|s| s.column == col);
            let label = match (pick, s) {
                (Ok(d), Some(s)) => {
                    s.dash = d;
                    "Series dash"
                }
                (Err(p), Some(s)) => {
                    s.point = p;
                    "Series point shape"
                }
                (_, None) => return,
            };
            self.chart_commit(id, label, next);
        }
    }

    fn chart_mark_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        match chart.kind {
            ChartKind::Column | ChartKind::Bar => {
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Group gap",
                        "Group gap",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.bars.group_gap,
                        |c, v| c.options.bars.group_gap = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Bar gap",
                        "Bar gap",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.bars.bar_gap,
                        |c, v| c.options.bars.bar_gap = v,
                    );
                });
                self.chart_number(
                    ui,
                    id,
                    chart,
                    ui.available_width(),
                    "Radius",
                    "Bar radius",
                    Scrub::whole(0.25).range(0.0..=100.0),
                    0,
                    |c| c.options.bars.radius,
                    |c, v| c.options.bars.radius = v,
                );
                let all = chart.options.bars.radius_all;
                self.chart_switch(ui, id, chart, "Round every corner", all, |c, on| {
                    c.options.bars.radius_all = on;
                });
            }
            ChartKind::Line | ChartKind::Area => {
                if chart.kind == ChartKind::Area {
                    let at = usize::from(chart.options.lines.curve == Curve::Smooth);
                    self.chart_segments(
                        ui,
                        id,
                        chart,
                        "Curve",
                        &["Straight", "Smooth"],
                        at,
                        |c, i| {
                            c.options.lines.curve = if i == 1 {
                                Curve::Smooth
                            } else {
                                Curve::Straight
                            }
                        },
                    );
                }
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Width",
                        "Line width",
                        Scrub::fine(0.1, 1).range(0.0..=40.0),
                        1,
                        |c| c.options.lines.width,
                        |c, v| c.options.lines.width = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Points",
                        "Point size",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.lines.point_size,
                        |c, v| c.options.lines.point_size = v,
                    );
                });
                let at = match chart.options.lines.points {
                    PointStyle::None => 0,
                    PointStyle::Hollow => 1,
                    PointStyle::Filled => 2,
                };
                self.chart_segments(
                    ui,
                    id,
                    chart,
                    "Points",
                    &["None", "Hollow", "Filled"],
                    at,
                    |c, i| {
                        c.options.lines.points =
                            [PointStyle::None, PointStyle::Hollow, PointStyle::Filled][i]
                    },
                );
                if chart.kind == ChartKind::Area {
                    self.chart_number(
                        ui,
                        id,
                        chart,
                        ui.available_width(),
                        "Opacity",
                        "Area opacity",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.lines.area_opacity * 100.0,
                        |c, v| c.options.lines.area_opacity = v / 100.0,
                    );
                }
                let gaps = chart.options.lines.connect_gaps;
                self.chart_switch(ui, id, chart, "Connect gaps", gaps, |c, on| {
                    c.options.lines.connect_gaps = on;
                });
            }
            ChartKind::Pie => {
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Hole",
                        "Donut hole",
                        Scrub::whole(0.5).range(0.0..=95.0),
                        0,
                        |c| c.options.slices.hole,
                        |c, v| c.options.slices.hole = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Pad",
                        "Slice gap",
                        Scrub::fine(0.1, 1).range(0.0..=45.0),
                        1,
                        |c| c.options.slices.pad_angle,
                        |c, v| c.options.slices.pad_angle = v,
                    );
                });
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Radius",
                        "Slice radius",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.slices.radius,
                        |c, v| c.options.slices.radius = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Start",
                        "Start angle",
                        Scrub::whole(1.0).range(-360.0..=360.0),
                        0,
                        |c| c.options.slices.start_angle,
                        |c, v| c.options.slices.start_angle = v,
                    );
                });
                let sort = chart.options.slices.sort;
                self.chart_switch(ui, id, chart, "Sort by value", sort, |c, on| {
                    c.options.slices.sort = on;
                });
                let group = chart.options.slices.group_below.is_some();
                self.chart_switch(ui, id, chart, "Group small slices", group, |c, on| {
                    c.options.slices.group_below = on.then_some(3.0);
                });
                if group {
                    self.chart_number(
                        ui,
                        id,
                        chart,
                        ui.available_width(),
                        "Below",
                        "Group small slices",
                        Scrub::fine(0.1, 1).range(0.0..=100.0),
                        1,
                        |c| c.options.slices.group_below.unwrap_or(3.0),
                        |c, v| c.options.slices.group_below = Some(v),
                    );
                }
                if self
                    .chart_card
                    .as_ref()
                    .is_some_and(|c| c.id == id && c.track)
                {
                    self.chart_slot_row(ui, id, chart, ChartColour::Track, "Track colour");
                }
            }
            ChartKind::Scatter => {
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Size",
                        "Point size",
                        Scrub::whole(0.25).range(0.0..=200.0),
                        0,
                        |c| c.options.points.size,
                        |c, v| c.options.points.size = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Largest",
                        "Largest bubble",
                        Scrub::whole(0.5).range(0.0..=400.0),
                        0,
                        |c| c.options.points.max_size,
                        |c, v| c.options.points.max_size = v,
                    );
                });
                self.chart_number(
                    ui,
                    id,
                    chart,
                    ui.available_width(),
                    "Opacity",
                    "Point opacity",
                    Scrub::whole(0.5).range(0.0..=100.0),
                    0,
                    |c| c.options.points.opacity * 100.0,
                    |c, v| c.options.points.opacity = v / 100.0,
                );
                let shapes = [
                    PointShape::Circle,
                    PointShape::Square,
                    PointShape::Diamond,
                    PointShape::Triangle,
                ];
                let at = shapes
                    .iter()
                    .position(|s| *s == chart.options.points.shape)
                    .unwrap_or(0);
                self.chart_segments(
                    ui,
                    id,
                    chart,
                    "Point shape",
                    &["Circle", "Square", "Diamond", "Triangle"],
                    at,
                    move |c, i| c.options.points.shape = shapes[i],
                );
            }
            ChartKind::Sankey => {
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Width",
                        "Node width",
                        Scrub::whole(0.25).range(1.0..=200.0),
                        0,
                        |c| c.options.flows.node_width,
                        |c, v| c.options.flows.node_width = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Gap",
                        "Node gap",
                        Scrub::whole(0.25).range(0.0..=200.0),
                        0,
                        |c| c.options.flows.node_gap,
                        |c, v| c.options.flows.node_gap = v,
                    );
                });
                self.chart_pair(ui, |app, ui, w| {
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Radius",
                        "Node radius",
                        Scrub::whole(0.25).range(0.0..=100.0),
                        0,
                        |c| c.options.flows.node_radius,
                        |c, v| c.options.flows.node_radius = v,
                    );
                    app.chart_number(
                        ui,
                        id,
                        chart,
                        w,
                        "Opacity",
                        "Flow opacity",
                        Scrub::whole(0.5).range(0.0..=100.0),
                        0,
                        |c| c.options.flows.opacity * 100.0,
                        |c, v| c.options.flows.opacity = v / 100.0,
                    );
                });
                let aligns = [
                    FlowAlign::Justify,
                    FlowAlign::Left,
                    FlowAlign::Right,
                    FlowAlign::Centre,
                ];
                let at = aligns
                    .iter()
                    .position(|a| *a == chart.options.flows.align)
                    .unwrap_or(0);
                self.chart_menu(
                    ui,
                    id,
                    chart,
                    "chart-flow-align",
                    "Alignment",
                    &["Justify", "Left", "Right", "Centre"],
                    at,
                    move |c, i| c.options.flows.align = aligns[i],
                );
                let at = usize::from(!chart.options.flows.relax);
                self.chart_segments(
                    ui,
                    id,
                    chart,
                    "Node order",
                    &["Automatic", "Table order"],
                    at,
                    |c, i| c.options.flows.relax = i == 0,
                );
                let colours = [
                    FlowColour::Source,
                    FlowColour::Target,
                    FlowColour::Gradient,
                    FlowColour::Neutral,
                ];
                let at = colours
                    .iter()
                    .position(|f| *f == chart.options.flows.colour)
                    .unwrap_or(0);
                self.chart_menu(
                    ui,
                    id,
                    chart,
                    "chart-flow-colour",
                    "Flow colour",
                    &["Source", "Target", "Gradient", "Neutral"],
                    at,
                    move |c, i| c.options.flows.colour = colours[i],
                );
                if chart.options.flows.colour == FlowColour::Neutral {
                    self.chart_slot_row(ui, id, chart, ChartColour::Neutral, "Neutral colour");
                }
                let labels = [NodeLabels::Name, NodeLabels::NameAndValue, NodeLabels::Off];
                let at = labels
                    .iter()
                    .position(|l| *l == chart.options.flows.labels)
                    .unwrap_or(0);
                self.chart_segments(
                    ui,
                    id,
                    chart,
                    "Node labels",
                    &["Name", "Name and value", "Off"],
                    at,
                    move |c, i| c.options.flows.labels = labels[i],
                );
            }
        }
        let outline = chart.options.outline;
        self.chart_switch(
            ui,
            id,
            chart,
            "Outline marks",
            outline.is_some(),
            |c, on| c.options.outline = on.then_some(chart::OUTLINE_WIDTH),
        );
        if outline.is_some() {
            self.chart_number(
                ui,
                id,
                chart,
                ui.available_width(),
                "Stroke",
                "Outline width",
                Scrub::fine(0.1, 1).range(0.1..=20.0),
                1,
                |c| c.options.outline.unwrap_or(chart::OUTLINE_WIDTH),
                |c, v| c.options.outline = Some(v.max(0.1)),
            );
        }
    }

    fn chart_axis_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let (first, second) = match chart.kind {
            ChartKind::Bar => ("Value axis (bottom)", "Category axis (left)"),
            ChartKind::Scatter => ("Y axis", "X axis"),
            _ => ("Value axis", "Category axis"),
        };
        for (value, title) in [(true, first), (false, second)] {
            ui.label(ui::eyebrow(title));
            let pick = move |c: &Chart| {
                if value {
                    c.options.value_axis.clone()
                } else {
                    c.options.category_axis.clone()
                }
            };
            let put = move |c: &mut Chart, a: chart::Axis| {
                if value {
                    c.options.value_axis = a
                } else {
                    c.options.category_axis = a
                }
            };
            let axis = pick(chart);
            let label = if value { "Value axis" } else { "Category axis" };
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Show value axis"
                } else {
                    "Show category axis"
                },
                axis.show,
                move |c, on| {
                    let mut a = pick(c);
                    a.show = on;
                    put(c, a);
                },
            );
            if !axis.show {
                continue;
            }
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Value axis line"
                } else {
                    "Category axis line"
                },
                axis.line,
                move |c, on| {
                    let mut a = pick(c);
                    a.line = on;
                    put(c, a);
                },
            );
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Value tick marks"
                } else {
                    "Category tick marks"
                },
                axis.ticks,
                move |c, on| {
                    let mut a = pick(c);
                    a.ticks = on;
                    put(c, a);
                },
            );
            if axis.line || axis.ticks {
                let name = if value {
                    "Value axis colour"
                } else {
                    "Category axis colour"
                };
                self.chart_slot_row(ui, id, chart, ChartColour::Axis(value), name);
            }
            self.chart_switch(
                ui,
                id,
                chart,
                if value {
                    "Value gridlines"
                } else {
                    "Category gridlines"
                },
                axis.grid.show,
                move |c, on| {
                    let mut a = pick(c);
                    a.grid.show = on;
                    put(c, a);
                },
            );
            if axis.grid.show {
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    if value {
                        "Dashed value gridlines"
                    } else {
                        "Dashed category gridlines"
                    },
                    axis.grid.dashed,
                    move |c, on| {
                        let mut a = pick(c);
                        a.grid.dashed = on;
                        put(c, a);
                    },
                );
                let name = if value {
                    "Value gridline colour"
                } else {
                    "Category gridline colour"
                };
                self.chart_slot_row(ui, id, chart, ChartColour::Grid(value), name);
            }
            let angles = [
                LabelAngle::Auto,
                LabelAngle::Flat,
                LabelAngle::Slanted,
                LabelAngle::Upright,
            ];
            let at = angles.iter().position(|a| *a == axis.angle).unwrap_or(0);
            self.chart_segments(
                ui,
                id,
                chart,
                if value {
                    "Value label angle"
                } else {
                    "Category label angle"
                },
                &["Auto", "0°", "45°", "90°"],
                at,
                move |c, i| {
                    let mut a = pick(c);
                    a.angle = angles[i];
                    put(c, a);
                },
            );
            let sides: &[&str] = match (chart.kind == ChartKind::Bar) == value {
                true => &["Bottom", "Top"],
                false => &["Left", "Right"],
            };
            let at = usize::from(axis.side == AxisSide::End);
            self.chart_segments(
                ui,
                id,
                chart,
                if value {
                    "Value axis side"
                } else {
                    "Category axis side"
                },
                sides,
                at,
                move |c, i| {
                    let mut a = pick(c);
                    a.side = if i == 1 {
                        AxisSide::End
                    } else {
                        AxisSide::Start
                    };
                    put(c, a);
                },
            );
            if value || chart.kind == ChartKind::Scatter {
                let zero_held = !matches!(chart.kind, ChartKind::Line | ChartKind::Scatter);
                if !zero_held {
                    self.chart_switch(
                        ui,
                        id,
                        chart,
                        if value {
                            "Start at zero"
                        } else {
                            "Start X at zero"
                        },
                        axis.start_at_zero,
                        move |c, on| {
                            let mut a = pick(c);
                            a.start_at_zero = on;
                            put(c, a);
                        },
                    );
                }
                let auto_range = axis.range.is_none();
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    if value { "Auto range" } else { "Auto X range" },
                    auto_range,
                    move |c, on| {
                        let mut a = pick(c);
                        a.range = (!on).then(|| auto_extent(c, value));
                        put(c, a);
                    },
                );
                if let Some((lo, hi)) = axis.range {
                    let step = ((hi - lo).abs() / 100.0).max(0.01);
                    let places = chart::scale::decimals_of(step).min(3);
                    let decimals = usize::from(places);
                    let scrub = Scrub::fine(step, u32::from(places));
                    self.chart_pair(ui, |app, ui, w| {
                        app.chart_number(
                            ui,
                            id,
                            chart,
                            w,
                            "Min",
                            if value { "Axis minimum" } else { "X minimum" },
                            scrub.clone(),
                            decimals,
                            move |c| pick(c).range.map_or(lo, |r| r.0),
                            move |c, v| {
                                let mut a = pick(c);
                                if let Some(r) = a.range.as_mut().filter(|r| v < r.1) {
                                    r.0 = v;
                                }
                                put(c, a);
                            },
                        );
                        app.chart_number(
                            ui,
                            id,
                            chart,
                            w,
                            "Max",
                            if value { "Axis maximum" } else { "X maximum" },
                            scrub,
                            decimals,
                            move |c| pick(c).range.map_or(hi, |r| r.1),
                            move |c, v| {
                                let mut a = pick(c);
                                if let Some(r) = a.range.as_mut().filter(|r| v > r.0) {
                                    r.1 = v;
                                }
                                put(c, a);
                            },
                        );
                    });
                }
                if value {
                    self.chart_format(ui, id, chart, "chart-value-format", |c| {
                        &mut c.options.value_axis.format
                    });
                } else {
                    self.chart_format(ui, id, chart, "chart-x-format", |c| {
                        &mut c.options.category_axis.format
                    });
                }
                let auto = axis.tick_count.is_none();
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    if value { "Auto ticks" } else { "Auto X ticks" },
                    auto,
                    move |c, on| {
                        let mut a = pick(c);
                        a.tick_count = (!on).then_some(5);
                        put(c, a);
                    },
                );
                if !auto {
                    self.chart_number(
                        ui,
                        id,
                        chart,
                        ui.available_width(),
                        "Ticks",
                        if value { "Tick count" } else { "X tick count" },
                        Scrub::whole(0.1).range(1.0..=100.0),
                        0,
                        move |c| f64::from(pick(c).tick_count.unwrap_or(5)),
                        move |c, v| {
                            let mut a = pick(c);
                            a.tick_count = Some(v.round().clamp(1.0, 100.0) as u32);
                            put(c, a);
                        },
                    );
                }
            }
            self.chart_axis_title(ui, id, chart, value, label);
        }
    }

    /// An axis's title, typed; committed when the field lets go.
    fn chart_axis_title(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        value: bool,
        label: &'static str,
    ) {
        let width = ui.available_width();
        self.chart_text(
            ui,
            id,
            chart,
            ("chart-axis-title", value),
            width,
            "Title",
            &format!("{label} title"),
            "Axis title",
            move |c| {
                if value {
                    &mut c.options.value_axis.title
                } else {
                    &mut c.options.category_axis.title
                }
            },
        );
    }

    /// A text option typed into a field `width` wide, committed as `label` when
    /// the field lets go; `slot` finds the text in a description.
    #[allow(clippy::too_many_arguments)]
    fn chart_text(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: impl Hash + std::fmt::Debug,
        width: f32,
        hint: &str,
        tip: &str,
        label: &'static str,
        slot: impl Fn(&mut Chart) -> &mut String,
    ) {
        let mut probe = chart.clone();
        let stored = slot(&mut probe).clone();
        let key = egui::Id::new(("chart-text", id, salt));
        let mut text = ui
            .data(|d| d.get_temp::<String>(key))
            .unwrap_or_else(|| stored.clone());
        let resp = ui::text_field(ui, egui::vec2(width, ui::CONTROL_H), &mut text, hint)
            .on_hover_text(tip);
        if resp.has_focus() {
            ui.data_mut(|d| d.insert_temp(key, text.clone()));
        } else {
            ui.data_mut(|d| d.remove::<String>(key));
        }
        if ui::defocus_commits(&resp) && text != stored {
            let mut next = chart.clone();
            *slot(&mut next) = text.chars().take(chart::MAX_TEXT_CHARS).collect();
            self.chart_commit(id, label, next);
        }
    }

    /// The *Format* dropdown over one number format: closed, a sample written
    /// through it; open, its prefix and suffix, decimals, and the three
    /// switches, the same rows whatever is set so the popup keeps its size.
    fn chart_format(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        chart: &Chart,
        salt: &'static str,
        slot: fn(&mut Chart) -> &mut chart::NumberFormat,
    ) {
        let mut probe = chart.clone();
        let f = slot(&mut probe).clone();
        let sample = chart::scale::format(FORMAT_SAMPLE, &f, 1);
        ui::labelled(ui, "Format", |ui| {
            egui::ComboBox::from_id_salt((salt, id))
                .icon(ui::combo_chevron)
                .width(ui.available_width())
                // The popup holds text fields, so a click inside must not dismiss it.
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .selected_text(ui::combo_text(&sample))
                .show_ui(ui, |ui| {
                    ui.set_width(FORMAT_POPUP_W);
                    ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                    let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                        self.chart_text(
                            ui,
                            id,
                            chart,
                            (salt, "prefix"),
                            half,
                            "Prefix",
                            "Written before the number, as $",
                            "Number prefix",
                            move |c| &mut slot(c).prefix,
                        );
                        self.chart_text(
                            ui,
                            id,
                            chart,
                            (salt, "suffix"),
                            half,
                            "Suffix",
                            "Written after the number, as kg",
                            "Number suffix",
                            move |c| &mut slot(c).suffix,
                        );
                    });
                    let at = f.decimals.map_or(0, |d| usize::from(d.min(3)) + 1);
                    ui.label(ui::eyebrow("Decimals"));
                    self.chart_segments(
                        ui,
                        id,
                        chart,
                        "Decimals",
                        &["Auto", "0", "1", "2", "3"],
                        at,
                        move |c, i| slot(c).decimals = i.checked_sub(1).map(|d| d as u8),
                    );
                    for (name, on, set) in [
                        (
                            "Thousands separator",
                            f.thousands,
                            (|f, on| f.thousands = on) as fn(&mut chart::NumberFormat, bool),
                        ),
                        ("Compact", f.compact, |f, on| f.compact = on),
                        ("Percent", f.percent, |f, on| f.percent = on),
                    ] {
                        self.chart_switch(ui, id, chart, name, on, move |c, on| {
                            set(slot(c), on);
                        });
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{} → {sample}",
                            chart::scale::format(FORMAT_SAMPLE, &chart::NumberFormat::default(), 1)
                        ))
                        .size(ui::SMALL_PT)
                        .color(theme::text::muted()),
                    );
                });
        });
    }

    fn chart_value_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        if chart.kind == ChartKind::Pie {
            let labels = [PieLabel::Value, PieLabel::Percent, PieLabel::Category];
            let at = labels
                .iter()
                .position(|l| *l == chart.options.values.pie)
                .unwrap_or(0);
            let off = chart.options.values.position == ValuePosition::Off;
            self.chart_switch(ui, id, chart, "Slice labels", !off, |c, on| {
                c.options.values.position = if on {
                    ValuePosition::Inside
                } else {
                    ValuePosition::Off
                };
            });
            if !off {
                self.chart_segments(
                    ui,
                    id,
                    chart,
                    "Slice label",
                    &["Value", "Percent", "Category"],
                    at,
                    move |c, i| c.options.values.pie = labels[i],
                );
                let outside = chart.options.values.pie_outside;
                self.chart_switch(
                    ui,
                    id,
                    chart,
                    "Outside with a leader line",
                    outside,
                    |c, on| {
                        c.options.values.pie_outside = on;
                    },
                );
            }
        } else {
            let places = [
                ValuePosition::Off,
                ValuePosition::Inside,
                ValuePosition::Outside,
                ValuePosition::End,
            ];
            let at = places
                .iter()
                .position(|p| *p == chart.options.values.position)
                .unwrap_or(0);
            self.chart_segments(
                ui,
                id,
                chart,
                "Value labels",
                &["Off", "Inside", "Outside", "End"],
                at,
                move |c, i| c.options.values.position = places[i],
            );
        }
        self.chart_format(ui, id, chart, "chart-values-format", |c| {
            &mut c.options.values.format
        });
    }

    fn chart_legend_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        let places = [
            LegendPosition::Off,
            LegendPosition::Top,
            LegendPosition::Bottom,
            LegendPosition::Left,
            LegendPosition::Right,
        ];
        let at = places
            .iter()
            .position(|p| *p == chart.options.legend.position)
            .unwrap_or(0);
        self.chart_segments(
            ui,
            id,
            chart,
            "Legend position",
            &["Off", "Top", "Bottom", "Left", "Right"],
            at,
            move |c, i| c.options.legend.position = places[i],
        );
        if chart.options.legend.position != LegendPosition::Off {
            let aligns = [Align::Start, Align::Centre, Align::End];
            let at = aligns
                .iter()
                .position(|a| *a == chart.options.legend.align)
                .unwrap_or(0);
            self.chart_segments(
                ui,
                id,
                chart,
                "Legend alignment",
                &["Start", "Centre", "End"],
                at,
                move |c, i| c.options.legend.align = aligns[i],
            );
        }
    }

    fn chart_text_rows(&mut self, ui: &mut egui::Ui, id: NodeId, chart: &Chart) {
        // Empty is the document's default family, which the chart is drawn in.
        let shown = match chart.options.text.family.as_str() {
            "" => ondin_core::typography::TextStyle::default().font_family,
            f => f.to_string(),
        };
        let head = if self.fonts.family_status(&shown) == crate::fonts::FamilyStatus::Missing {
            ui::glyph_and_text_tinted(icon::TEXT_AA, &shown, color::warn())
        } else {
            ui::glyph_and_text(icon::TEXT_AA, &shown)
        };
        let (picked, _) = self.family_dropdown(
            ui,
            ("chart-family", id),
            egui::vec2(ui.available_width(), ui::CONTROL_H),
            head,
            &shown,
        );
        if let Some(family) = picked.filter(|f| *f != shown) {
            self.fonts.ensure_loaded(&family);
            let mut next = chart.clone();
            next.options.text.family = family;
            self.chart_commit(id, "Chart font", next);
        }
        self.chart_slot_row(ui, id, chart, ChartColour::Ink, "Text colour");
        self.chart_pair(ui, |app, ui, w| {
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Ticks",
                "Tick label size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.tick_size,
                |c, v| c.options.text.tick_size = v,
            );
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Values",
                "Value label size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.value_size,
                |c, v| c.options.text.value_size = v,
            );
        });
        self.chart_pair(ui, |app, ui, w| {
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Legend",
                "Legend size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.legend_size,
                |c, v| c.options.text.legend_size = v,
            );
            app.chart_number(
                ui,
                id,
                chart,
                w,
                "Titles",
                "Axis title size",
                Scrub::fine(0.1, 1).range(1.0..=200.0),
                1,
                |c| c.options.text.title_size,
                |c, v| c.options.text.title_size = v,
            );
        });
        self.chart_number(
            ui,
            id,
            chart,
            ui.available_width(),
            "Weight",
            "Chart text weight",
            Scrub::whole(2.0).range(1.0..=1000.0),
            0,
            |c| f64::from(c.options.text.weight),
            |c, v| c.options.text.weight = v.round().clamp(1.0, 1000.0) as u16,
        );
    }

    /// *Flatten chart*: each chart keeps its parts and drops its description, one
    /// undo step.
    pub(crate) fn flatten_charts(&mut self, ids: &[NodeId]) {
        let ops: Vec<Operation> = ids
            .iter()
            .filter_map(|id| chart::flatten(&self.session.doc, *id))
            .collect();
        if ops.is_empty() {
            return;
        }
        let n = ops.len();
        self.session.name_step("Flatten chart", icon::STACK_SIMPLE);
        if self.commit_edit(Transaction(ops)) {
            self.session.info(match n {
                1 => "Flattened the chart; its shapes are layers now".to_string(),
                n => format!("Flattened {n} charts; their shapes are layers now"),
            });
        }
    }
}

/// The colour variables a chart can take *From variables*, by group (the name
/// before the last `/`, else the collection's name), each group's variables in
/// the table's order: those that may bind a fill.
fn colour_groups(doc: &ondin_core::Document) -> Vec<(String, Vec<ondin_core::VarId>)> {
    use ondin_core::variable::{Field, VarKind, split_name};
    let t = doc.variables();
    let mut out: Vec<(String, Vec<ondin_core::VarId>)> = Vec::new();
    for v in t
        .iter()
        .filter(|v| v.kind == VarKind::Color && v.binds(Field::Fill))
    {
        let (group, _) = split_name(&v.name);
        let name = match group {
            "" => t
                .collection_of(v.id)
                .map_or_else(|| "Colours".to_string(), |c| c.name.clone()),
            g => g.to_string(),
        };
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, vars)) => vars.push(v.id),
            None => out.push((name, vec![v.id])),
        }
    }
    out
}

/// The range an axis starts at when *Auto range* is turned off: its data's
/// extent on nice steps, with zero where the kind or *Start at zero* holds it,
/// so the chart looks as it did until a bound is typed. `value` is the value
/// axis; the other is a scatter's X.
fn auto_extent(chart: &Chart, value: bool) -> (f64, f64) {
    let t = &chart.table;
    let numbers = |col: chart::ColumnId| -> Vec<Option<f64>> {
        t.index_of(col).map_or_else(Vec::new, |i| {
            t.rows.iter().map(|r| chart::parse_number(&r[i])).collect()
        })
    };
    let shown: Vec<chart::ColumnId> = chart
        .mapping
        .series
        .iter()
        .copied()
        .filter(|c| {
            chart
                .series
                .iter()
                .find(|s| s.column == *c)
                .is_none_or(|s| s.visible)
        })
        .collect();
    let o = &chart.options;
    let stacking = match chart.kind {
        ChartKind::Column | ChartKind::Bar => o.bars.stacking,
        ChartKind::Area => o.lines.area_stacking,
        _ => Stacking::Apart,
    };
    let mut seen: Vec<f64> = Vec::new();
    if !value {
        seen.extend(
            chart
                .mapping
                .category
                .map_or_else(Vec::new, numbers)
                .into_iter()
                .flatten(),
        );
    } else if stacking == Stacking::Percent {
        seen.extend([0.0, 1.0]);
    } else if stacking == Stacking::Stacked {
        let mut ups = vec![0.0; t.rows.len()];
        let mut downs = vec![0.0; t.rows.len()];
        for col in &shown {
            for (r, v) in numbers(*col).into_iter().enumerate() {
                match v {
                    Some(v) if v >= 0.0 => ups[r] += v,
                    Some(v) => downs[r] += v,
                    None => {}
                }
            }
        }
        seen.extend(ups.into_iter().chain(downs));
    } else {
        for col in &shown {
            seen.extend(numbers(*col).into_iter().flatten());
        }
    }
    let axis = if value {
        &o.value_axis
    } else {
        &o.category_axis
    };
    let zero = axis.start_at_zero
        || (value && !matches!(chart.kind, ChartKind::Line | ChartKind::Scatter));
    if zero {
        seen.push(0.0);
    }
    let lo = seen.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = seen.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let ticks = chart::scale::ticks(lo, hi, axis.tick_count.unwrap_or(5), None);
    (ticks.min, ticks.max)
}

/// Series `col` shown or hidden: a shown column not yet mapped joins the
/// series; a hidden one keeps its place and its colour.
fn show_series(chart: &mut Chart, col: chart::ColumnId, on: bool) {
    if on && !chart.mapping.series.contains(&col) {
        chart.mapping.series.push(col);
    }
    if let Some(s) = chart.series.iter_mut().find(|s| s.column == col) {
        s.visible = on;
    } else if !on && let Some(fill) = chart::series_fill(chart, col) {
        *chart = chart::with_series_fill(chart, col, fill);
        if let Some(s) = chart.series.iter_mut().find(|s| s.column == col) {
            s.visible = false;
        }
    }
}

/// A Style cell: the preset's picture over its name, the lit one in the accent,
/// grounded as `inspector::template_cell`'s cells are.
fn style_cell(
    ui: &mut egui::Ui,
    width: f32,
    name: &str,
    picture: Option<egui::TextureId>,
    on: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(width, STYLE_CELL_H), egui::Sense::click());
    let r5 = egui::CornerRadius::same(ui::BUTTON_R);
    let ground = match (on, resp.hovered()) {
        (true, false) => Some(color::accent_900()),
        (true, true) => Some(color::accent_800()),
        (false, true) => Some(theme::color::hover()),
        (false, false) => None,
    };
    let p = ui.painter();
    if let Some(bg) = ground {
        p.rect_filled(rect, r5, bg);
    }
    if on {
        p.rect_stroke(
            rect,
            r5,
            egui::Stroke::new(1.0, color::accent_700()),
            egui::StrokeKind::Inside,
        );
    }
    let frame = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + 6.0 + STYLE_PICTURE.y / 2.0),
        STYLE_PICTURE,
    );
    p.rect_filled(frame, egui::CornerRadius::same(2), egui::Color32::WHITE);
    if let Some(t) = picture {
        p.image(t, frame, crate::thumbs::FULL_UV, egui::Color32::WHITE);
    }
    let ink = if on {
        color::accent_200()
    } else {
        theme::text::muted()
    };
    p.text(
        egui::pos2(rect.center().x, rect.bottom() - 8.0),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(ui::SMALL_PT),
        ink,
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chart on bare canvas, selected, in a headless app.
    fn charted(ctx: &egui::Context, kind: ChartKind) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let id = app.session.ids.mint();
        let page = app.session.doc.first_page();
        let index = app.session.doc.get(page).map_or(0, |n| n.children().len());
        let tx = ondin_core::edit::create_chart(
            id,
            page,
            index,
            Affine::IDENTITY,
            kurbo::Point::new(-4000.0, 0.0),
            kurbo::Point::new(-3520.0, 280.0),
            ondin_core::peniko::Color::WHITE,
            Chart::sample(kind),
        );
        assert!(app.session.commit(tx));
        app.session.selection.set_one(id);
        (app, id)
    }

    fn pass(ctx: &egui::Context, app: &mut OndinApp) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    /// A selected chart shows the Chart card in place of the frame's Container,
    /// Layout grid and Frame templates cards, with a picture in every Style cell
    /// and *Clean* the lit one on a new chart.
    /// flip: `inspector_single` drawing the Container card for a chart → fails at "no Container card".
    #[test]
    fn a_chart_shows_the_chart_card_and_not_the_frames() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        let shown = &app.cards_shown;
        assert!(shown.contains(&"Chart"), "the Chart card: {shown:?}");
        for frame_card in ["Container", "Layout grid", "Frame templates"] {
            assert!(
                !shown.contains(&frame_card),
                "no {frame_card} card: {shown:?}"
            );
        }
        let cache = app
            .chart_card
            .as_ref()
            .expect("the card measured its chart");
        assert_eq!(cache.id, id);
        assert_eq!(
            cache.thumbs.len(),
            Preset::ALL.len(),
            "a picture per Style cell"
        );
        assert!(cache.left_out.is_empty() && cache.refusal.is_none());
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(Preset::of(&chart.options), Some(Preset::Clean));
    }

    /// A series colour written through the picker's slot reads back from the
    /// slot, and the commit leaves the other series on the palette.
    #[test]
    fn a_series_colour_goes_through_the_pickers_slot() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Line);
        let chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        let (first, second) = (chart.mapping.series[0], chart.mapping.series[1]);
        let red = ondin_core::peniko::Color::from_rgb8(255, 0, 0);
        let slot = PaintSlot::Chart(ChartColour::Series(second));
        let tx = app.slot_transaction(id, slot, Brush::Solid(red));
        assert!(app.commit_edit(tx));
        assert_eq!(app.slot_brush(id, slot), Some(Brush::Solid(red)));
        let after = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(
            chart::series_fill(after, first),
            chart::series_fill(&chart, first),
            "the other series kept its colour"
        );
    }

    /// `Ctrl+E` on a selected chart flattens it: the frame keeps its parts as
    /// layers and loses its description, in one undo step.
    #[test]
    fn ctrl_e_flattens_a_chart() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Pie);
        let parts = app.session.doc.get(id).map_or(0, |n| n.children().len());
        app.flatten_selection();
        let node = app.session.doc.get(id).expect("the frame stays");
        assert!(node.chart().is_none(), "the description is gone");
        assert_eq!(node.children().len(), parts, "the parts stay");
        app.session.undo();
        assert!(app.session.doc.get(id).and_then(|n| n.chart()).is_some());
    }

    /// Two charts of one kind selected show one Chart card, and an edit through
    /// it writes both in one step, each keeping its own other options.
    /// flip: `chart_tx` skipping `chart_others` → fails at "the second chart took the radius".
    #[test]
    fn a_multi_selection_of_charts_edits_each() {
        let ctx = egui::Context::default();
        let (mut app, a) = charted(&ctx, ChartKind::Column);
        let b = app.session.ids.mint();
        let page = app.session.doc.first_page();
        let index = app.session.doc.get(page).map_or(0, |n| n.children().len());
        let mut own = Chart::sample(ChartKind::Column);
        own.options.bars.group_gap = 55.0;
        let tx = ondin_core::edit::create_chart(
            b,
            page,
            index,
            Affine::IDENTITY,
            kurbo::Point::new(-4000.0, 400.0),
            kurbo::Point::new(-3520.0, 680.0),
            ondin_core::peniko::Color::WHITE,
            own,
        );
        assert!(app.session.commit(tx));
        app.session.selection.set_one(a);
        app.session.selection.add(b);
        for _ in 0..3 {
            pass(&ctx, &mut app);
        }
        assert!(app.cards_shown.contains(&"Chart"), "{:?}", app.cards_shown);
        assert!(!app.cards_shown.contains(&"Container"));
        assert_eq!(app.selected_charts(), Some(vec![a, b]));

        let lead = app
            .session
            .doc
            .get(a)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        let mut next = lead.clone();
        next.options.bars.radius = 9.0;
        app.chart_others = vec![b];
        app.chart_commit(a, "Bar radius", next);
        app.chart_others.clear();
        let chart = |app: &OndinApp, id| {
            app.session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .cloned()
                .unwrap()
        };
        assert_eq!(chart(&app, a).options.bars.radius, 9.0);
        assert_eq!(
            chart(&app, b).options.bars.radius,
            9.0,
            "the second chart took the radius"
        );
        assert_eq!(
            chart(&app, b).options.bars.group_gap,
            55.0,
            "and kept its own gap"
        );
        app.session.undo();
        assert_eq!(
            chart(&app, b).options.bars.radius,
            lead.options.bars.radius,
            "one step"
        );
    }

    /// *Auto range* turned off starts at the range the data already draws: zero
    /// to a nice step past the largest value, or past the largest stack.
    #[test]
    fn auto_range_off_starts_at_the_datas_extent() {
        let mut chart = Chart::sample(ChartKind::Column);
        let most = chart
            .table
            .rows
            .iter()
            .flat_map(|r| r.iter().filter_map(|c| chart::parse_number(c)))
            .fold(0.0, f64::max);
        let (lo, hi) = auto_extent(&chart, true);
        assert_eq!(lo, 0.0);
        assert!(hi >= most && hi < most * 2.0, "{hi} for {most}");
        chart.options.bars.stacking = Stacking::Stacked;
        let (_, stacked) = auto_extent(&chart, true);
        assert!(stacked > hi, "a stack reaches past one series: {stacked}");
    }

    /// The Type section's variants round-trip: each segment written reads back
    /// as itself.
    #[test]
    fn every_variant_reads_back_as_itself() {
        for kind in ChartKind::ALL {
            let chart = Chart::sample(kind);
            let Some((names, _)) = variants(&chart) else {
                continue;
            };
            for i in 0..names.len() {
                let next = with_variant(&chart, i);
                assert_eq!(variants(&next).map(|(_, at)| at), Some(i), "{kind:?} {i}");
            }
        }
    }

    /// One frame of the app, its shapes returned; tall enough that the whole
    /// card is on screen, since a label out of view is not painted.
    fn shapes(ctx: &egui::Context, app: &mut OndinApp) -> Vec<egui::epaint::ClippedShape> {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 4000.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
            .shapes
    }

    /// Whether a text shape reading `s` is among `shapes`.
    fn drawn(shapes: &[egui::epaint::ClippedShape], s: &str) -> bool {
        fn find(shape: &egui::Shape, s: &str) -> bool {
            match shape {
                egui::Shape::Text(t) => t.galley.text() == s,
                egui::Shape::Vec(v) => v.iter().any(|x| find(x, s)),
                _ => false,
            }
        }
        shapes.iter().any(|c| find(&c.shape, s))
    }

    /// A colour variable `name` holding `c` in a new mode of the first
    /// collection; returns it and the mode.
    fn colour_var(
        app: &mut OndinApp,
        name: &str,
        c: ondin_core::peniko::Color,
    ) -> (ondin_core::VarId, ondin_core::variable::ModeId) {
        use ondin_core::variable::{Cell, CollectionId, Mode, VarKind, Variable};
        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        assert!(app.commit_edit(Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: name.into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(c))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        (var, mode)
    }

    /// Each axis and gridline slot writes its own colour and no other; the
    /// Sankey's neutral and the ring's track are the value axis's and the
    /// value gridlines'.
    /// flip: `with_fill` writing `Axis(false)` to the value axis → fails at "the category axis".
    #[test]
    fn axis_and_gridline_slots_write_their_own_colour() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let red = ondin_core::peniko::Color::from_rgb8(255, 0, 0);
        let lit =
            |c: ondin_core::peniko::Color| -> ondin_core::BrushSource { Brush::Solid(c).into() };
        let before = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        let tx = app.slot_transaction(
            id,
            PaintSlot::Chart(ChartColour::Axis(false)),
            Brush::Solid(red),
        );
        assert!(app.commit_edit(tx));
        let o = &app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .options;
        assert_eq!(o.category_axis.colour, lit(red), "the category axis");
        assert_eq!(o.value_axis.colour, before.options.value_axis.colour);
        assert_eq!(
            o.category_axis.grid.colour,
            before.options.category_axis.grid.colour
        );
        let blue = ondin_core::peniko::Color::from_rgb8(0, 0, 255);
        let tx = app.slot_transaction(
            id,
            PaintSlot::Chart(ChartColour::Grid(true)),
            Brush::Solid(blue),
        );
        assert!(app.commit_edit(tx));
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(
            chart.options.value_axis.grid.colour,
            lit(blue),
            "the value gridlines"
        );
        assert_eq!(chart.options.category_axis.colour, lit(red), "kept");
        assert_eq!(fill_of(chart, ChartColour::Track), Some(lit(blue)));
        assert_eq!(
            fill_of(chart, ChartColour::Neutral),
            Some(chart.options.value_axis.colour.clone())
        );
    }

    /// The Axes section offers an axis's colour while it draws a line or ticks
    /// and its gridlines' colour while they show; a Sankey on Neutral flows
    /// offers its neutral colour, and a progress ring its track's.
    /// flip: `chart_axis_rows` dropping the gridline colour row → fails at "Value gridline colour".
    #[test]
    fn the_card_offers_axis_gridline_neutral_and_track_colours() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        app.collapsed_panels.remove("Chart axes");
        app.collapsed_panels.remove("Chart marks");
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        for row in ["Value gridline colour", "Category axis colour"] {
            assert!(drawn(&out, row), "{row}");
        }
        // A new chart's value axis has no line or ticks, its category axis no gridlines.
        for row in [
            "Value axis colour",
            "Category gridline colour",
            "Track colour",
            "Neutral colour",
        ] {
            assert!(!drawn(&out, row), "no {row}");
        }
        let mut chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        chart.options.value_axis.ticks = true;
        app.chart_commit(id, "Value tick marks", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Value axis colour"), "the ticks' colour");

        let (mut app, id) = charted(&ctx, ChartKind::Sankey);
        app.collapsed_panels.remove("Chart marks");
        let mut chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        chart.options.flows.colour = FlowColour::Neutral;
        app.chart_commit(id, "Flow colour", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Neutral colour"), "Neutral colour");

        let (mut app, id) = charted(&ctx, ChartKind::Pie);
        app.collapsed_panels.remove("Chart marks");
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(
            !drawn(&out, "Track colour"),
            "a pie of several slices has no track"
        );
        let mut chart = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .cloned()
            .unwrap();
        chart.table.rows.truncate(1);
        chart.table.rows[0][1] = "64".into();
        app.chart_commit(id, "Edit chart data", chart);
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "Track colour"), "a ring's track");
    }

    /// A bound series row names its variable in a capsule beside the series name.
    /// flip: `chart_colour_row` skipping the capsule → fails at "the variable's name".
    #[test]
    fn a_bound_series_row_names_its_variable() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let (var, _) = colour_var(&mut app, "brand/500", ondin_core::peniko::Color::WHITE);
        let col = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .mapping
            .series[0];
        let tx = app.chart_bind_tx(id, ChartColour::Series(col), Some(var), None);
        assert!(app.commit_edit(tx));
        assert_eq!(
            app.chart_binding(id, ChartColour::Series(col)).map(|b| b.0),
            Some(var)
        );
        let mut out = Vec::new();
        for _ in 0..3 {
            out = shapes(&ctx, &mut app);
        }
        assert!(drawn(&out, "brand/500"), "the variable's name");
    }

    /// The Style cells redraw when a variable a series is bound to changes value.
    /// flip: `refresh_chart_cache` keying on the description as written, not as
    /// resolved → fails at "redrawn".
    #[test]
    fn style_pictures_follow_a_bound_variables_value() {
        let ctx = egui::Context::default();
        let (mut app, id) = charted(&ctx, ChartKind::Column);
        let (var, mode) = colour_var(&mut app, "brand/500", ondin_core::peniko::Color::WHITE);
        let col = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .mapping
            .series[0];
        let tx = app.chart_bind_tx(id, ChartColour::Series(col), Some(var), None);
        assert!(app.commit_edit(tx));
        for _ in 0..3 {
            shapes(&ctx, &mut app);
        }
        let key = |app: &OndinApp| app.chart_card.as_ref().map(|c| c.key);
        let was = key(&app);
        assert!(was.is_some(), "the card measured its chart");
        let red = ondin_core::peniko::Color::from_rgb8(255, 0, 0);
        assert!(
            app.commit_edit(Transaction(vec![Operation::SetVariableCell {
                id: var,
                mode,
                cell: ondin_core::variable::Cell::Color(red),
            }]))
        );
        for _ in 0..3 {
            shapes(&ctx, &mut app);
        }
        assert_ne!(key(&app), was, "redrawn");
    }
}
