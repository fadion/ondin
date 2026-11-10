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

/// The colour a chart slot draws in, solid.
pub(crate) fn colour_of(chart: &Chart, colour: ChartColour) -> Option<ondin_core::peniko::Color> {
    let fill = match colour {
        ChartColour::Series(col) => chart::series_fill(chart, col)?,
        ChartColour::Node(i) => chart::sankey_nodes(chart).into_iter().nth(i)?.1,
    };
    match fill.literal() {
        Some(Brush::Solid(c)) => Some(*c),
        _ => None,
    }
}

/// Series `col`'s colour, solid: the data window's header swatch.
pub(crate) fn series_colour(
    chart: &Chart,
    col: chart::ColumnId,
) -> Option<ondin_core::peniko::Color> {
    colour_of(chart, ChartColour::Series(col))
}

/// `chart` with the slot's colour set to `c`.
pub(crate) fn with_colour(
    chart: &Chart,
    colour: ChartColour,
    c: ondin_core::peniko::Color,
) -> Chart {
    let fill = ondin_core::BrushSource::Literal(Brush::Solid(c));
    match colour {
        ChartColour::Series(col) => chart::with_series_fill(chart, col, fill),
        ChartColour::Node(i) => match chart::sankey_nodes(chart).into_iter().nth(i) {
            Some((name, _)) => chart::with_node_fill(chart, &name, fill),
            None => chart.clone(),
        },
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

/// A count with thousands separators, as the card's sentences write one.
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

impl OndinApp {
    /// The Chart card, for a chart frame.
    pub(super) fn inspector_chart(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(chart) = self.session.chart_drawn(id).cloned() else {
            return;
        };
        self.refresh_chart_cache(ui.ctx(), id, &chart);
        self.panel(ui, "Chart", None, |app, ui| {
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            app.chart_section(ui, "Chart type", "Type", |app, ui| {
                app.chart_type_rows(ui, id, &chart);
            });
            app.chart_section(ui, "Chart style", "Style", |app, ui| {
                app.chart_style_rows(ui, id, &chart);
            });
            app.chart_section(ui, "Chart data", "Data", |app, ui| {
                app.chart_data_rows(ui, id, &chart);
            });
            let series = if chart.kind == ChartKind::Sankey {
                "Nodes"
            } else {
                "Series"
            };
            app.chart_section(ui, "Chart series", series, |app, ui| {
                app.chart_series_rows(ui, id, &chart);
            });
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
                app.flatten_charts(&[id]);
            }
        });
    }

    /// Rebuild [`ChartCardCache`] when the description or the drawn size moved.
    fn refresh_chart_cache(&mut self, ctx: &egui::Context, id: NodeId, chart: &Chart) {
        let size = match self.session.resolved.used_kind(&self.session.doc, id) {
            Some(ondin_core::NodeKind::Artboard { size, .. }) => *size,
            _ => return,
        };
        let key = description_key(id, chart, size);
        if self
            .chart_card
            .as_ref()
            .is_some_and(|c| c.id == id && c.key == key)
        {
            return;
        }
        let family = ondin_core::typography::TextStyle::default().font_family;
        let (left_out, refusal) = match chart::generate(chart, size, &family) {
            Ok(g) => (g.left_out, None),
            Err(chart::Refusal::TooManyMarks { count, what }) => (
                Vec::new(),
                Some(format!(
                    "Too many marks: {} {what}, at most {}",
                    grouped(count),
                    grouped(chart::MAX_MARKS)
                )),
            ),
            Err(chart::Refusal::LoopsBack(path)) => {
                (Vec::new(), Some(format!("Flows loop back: {path}")))
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
        self.commit_edit(Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(next)),
        }]));
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
        let tx = Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(next)),
        }]);
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
        let at = palettes
            .iter()
            .position(|(p, _)| *p == chart.options.palette)
            .unwrap_or(0);
        let names: Vec<&str> = palettes.iter().map(|(_, n)| *n).collect();
        self.chart_menu(
            ui,
            id,
            chart,
            "chart-palette",
            "Palette",
            &names,
            at,
            |c, i| {
                *c = chart::with_palette(c, palettes[i].0);
            },
        );
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
        for (slot, name, shown) in rows {
            let colour = colour_of(chart, slot).map(ui::color_to_egui);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let field = w - if shown.is_some() {
                    ui::CONTROL_H + ui::CARD_COL_GAP
                } else {
                    0.0
                };
                ui::field_row(ui, egui::vec2(field, ui::CONTROL_H), |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    let chip = match (colour, shown) {
                        (_, Some(false)) | (None, _) => ui::Swatch::Mixed,
                        (Some(c), _) => ui::Swatch::Solid(c),
                    };
                    let resp = ui::swatch(ui, 14.0, theme::color::field(), chip);
                    if resp.clicked() && shown != Some(false) {
                        self.open_picker(ui, id, PaintSlot::Chart(slot), resp.rect.left_top());
                    }
                    let ink = match shown {
                        Some(false) => theme::text::off(),
                        _ => color::text(),
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&name).size(ui::CONTROL_PT).color(ink),
                        )
                        .truncate(),
                    );
                });
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
        let stored = if value {
            &chart.options.value_axis.title
        } else {
            &chart.options.category_axis.title
        };
        let key = egui::Id::new(("chart-axis-title", id, value));
        let mut text = ui
            .data(|d| d.get_temp::<String>(key))
            .unwrap_or_else(|| stored.clone());
        let resp = ui::text_field(
            ui,
            egui::vec2(ui.available_width(), ui::CONTROL_H),
            &mut text,
            "Title",
        )
        .on_hover_text(format!("{label} title"));
        if resp.has_focus() {
            ui.data_mut(|d| d.insert_temp(key, text.clone()));
        } else {
            ui.data_mut(|d| d.remove::<String>(key));
        }
        if ui::defocus_commits(&resp) && text != *stored {
            let mut next = chart.clone();
            let title: String = text.chars().take(chart::MAX_TEXT_CHARS).collect();
            if value {
                next.options.value_axis.title = title;
            } else {
                next.options.category_axis.title = title;
            }
            self.chart_commit(id, "Axis title", next);
        }
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
        let f = &chart.options.values.format;
        self.chart_switch(
            ui,
            id,
            chart,
            "Thousands separator",
            f.thousands,
            |c, on| {
                c.options.values.format.thousands = on;
            },
        );
        self.chart_switch(ui, id, chart, "Compact (1.2k)", f.compact, |c, on| {
            c.options.values.format.compact = on;
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
}
