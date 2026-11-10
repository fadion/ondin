//! A chart's table from a file (`charts.md`, *Import and the linked file*): the
//! import box, the link a chart keeps to its file, the watch that notices the
//! file change, and *Update*, *Relink* and *Unlink*.
//!
//! The path is the document's, so untrusted: it is looked at and read only by
//! the tokens link's rules (`tokens_import::local_path`, `read_linked`), and a
//! file's contents enter the document only on the user's click.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::chart::{self, Chart, ChartKind, DataLink, table_io};
use ondin_core::{NodeId, Operation, Transaction};
use std::path::{Path, PathBuf};

/// The box's width.
const W: f32 = 620.0;
const PAD: f32 = 14.0;
/// The rows the box shows of the table, and a preview column's width.
const PREVIEW_ROWS: usize = 20;
const PREVIEW_COL_W: f32 = 104.0;
const PREVIEW_ROW_H: f32 = 22.0;

/// The import box while it is open.
pub(crate) struct ChartImport {
    /// The chart the table replaces.
    chart: NodeId,
    path: PathBuf,
    /// Whether the file came with a path that can be linked (a browser drop
    /// brings only bytes).
    linkable: bool,
    text: String,
    format: table_io::Format,
    /// Whether a CSV's or TSV's first row is the header.
    header: bool,
    read: table_io::Read,
    link: bool,
}

/// What the watch last saw of the linked files: by path, the stat on disk.
#[derive(Default)]
pub(crate) struct ChartWatch {
    next: f64,
    focused: bool,
    /// Every linked path and the stat it has on disk now; `None` when it is not
    /// there, or not a file the link may read.
    seen: Vec<(String, Option<(u64, u64)>)>,
}

/// Every chart in `doc`, on every page, with its description.
fn charts(doc: &ondin_core::Document) -> Vec<(NodeId, &Chart)> {
    let roots: Vec<NodeId> = doc.pages().iter().map(|p| p.id).collect();
    ondin_core::build::subtree_nodes(doc, &roots)
        .into_iter()
        .filter_map(|id| Some((id, doc.get(id)?.chart()?)))
        .collect()
}

/// The stat a link stores: modified seconds and size.
fn stat_of(path: &Path) -> Option<(u64, u64)> {
    crate::tokens_import::on_disk(path)
}

/// A link's file name, for the pill and the status line.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// What a linked chart's file is now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LinkState {
    Current,
    Changed,
    Missing,
}

/// The kind a table dropped on bare canvas makes: a scatter when its first
/// column is numeric, a Sankey when it is exactly two text columns and a
/// number column, else columns.
pub(crate) fn kind_for(table: &chart::Table) -> ChartKind {
    let numeric: Vec<bool> = table
        .columns
        .iter()
        .map(|c| table.is_numeric(c.id))
        .collect();
    match numeric.as_slice() {
        [true, ..] => ChartKind::Scatter,
        [false, false, true] => ChartKind::Sankey,
        _ => ChartKind::Column,
    }
}

impl OndinApp {
    /// A new chart of `chart`'s description centred on `at`, at the click size, in
    /// whatever frame is drawn there; selected, with the data window on it. Undo
    /// step *Insert chart*.
    pub(crate) fn insert_chart_at(&mut self, at: ondin_core::kurbo::Point, chart: Chart) {
        let (w, h) = crate::tools::CHART_CLICK_SIZE;
        let half = ondin_core::kurbo::Vec2::new(w / 2.0, h / 2.0);
        let (parent, parent_world) = self.draw_target(at);
        let id = self.session.ids.mint();
        let index = self.child_count(parent);
        let tx = crate::tools::create_chart(
            id,
            parent,
            index,
            parent_world,
            at - half,
            at + half,
            ondin_core::peniko::Color::WHITE,
            chart,
        );
        self.session.name_step("Insert chart", icon::CHART);
        if self.commit_edit(tx) {
            self.session.selection.set_one(id);
            self.open_chart_data(id, (0, 0));
        }
    }

    /// *Import…*: pick a file for chart `id` and open the import box on it.
    pub(crate) fn import_chart_data_dialog(&mut self, id: NodeId) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Chart data", &["csv", "tsv", "json"])
            .pick_file()
        else {
            return;
        };
        self.open_chart_import(id, path, None);
    }

    /// Open the import box for chart `id` on the file at `path`, or on `bytes`
    /// when the file came without one. A file that does not read is refused on
    /// the status line.
    pub(crate) fn open_chart_import(&mut self, id: NodeId, path: PathBuf, bytes: Option<Vec<u8>>) {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let Some(format) = table_io::Format::of_name(&name) else {
            return self
                .session
                .fail(format!("Cannot import {name}: not a CSV, TSV or JSON file"));
        };
        let linkable = bytes.is_none();
        let text = match bytes {
            Some(b) => String::from_utf8_lossy(&b).into_owned(),
            None => {
                match crate::tokens_import::read_linked(&path, crate::tokens_import::MAX_READ) {
                    Some(t) => t,
                    None => {
                        return self.session.fail(format!(
                            "Cannot import {name}: not a local file of at most 16 MB"
                        ));
                    }
                }
            }
        };
        let read = match table_io::read(&text, format, None) {
            Ok(r) => r,
            Err(e) => return self.session.fail(format!("Cannot import {name}: {e}")),
        };
        self.chart_import = Some(ChartImport {
            chart: id,
            path,
            linkable,
            text,
            format,
            header: read.header,
            read,
            link: linkable,
        });
    }

    /// Replace chart `id`'s table with `read`'s, linked to `link` or unlinked,
    /// as one undo step named `step`; says what it did.
    fn replace_chart_table(
        &mut self,
        id: NodeId,
        table: chart::Table,
        link: Option<DataLink>,
        step: &'static str,
    ) -> bool {
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()) else {
            return false;
        };
        let next = Chart {
            source: link,
            ..chart.with_table(table)
        };
        self.session.name_step(step, icon::IMPORT);
        self.commit_edit(Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(next)),
        }]))
    }

    /// Draw the open box and answer its keys and clicks: a modal with a clear
    /// backdrop, as the tokens review's.
    pub(crate) fn chart_import_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut t) = self.chart_import.take() else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        let enter = ui::card_enter(ctx);
        let mut go: Option<bool> = None;
        let mut reread = false;
        let hold = crate::settings::CardHold::Modal;
        let clicked_away =
            crate::settings::canvas_card(ctx, "chart-import", canvas, W, hold, |ui| {
                let (name, dirs) = crate::tokens_import::where_label(&t.path);
                let place = match dirs.is_empty() || !t.linkable {
                    true => name,
                    false => format!("{name} \u{b7} {dirs}"),
                };
                if crate::settings::card_head_noted(ui, W, "Import chart data", Some(&place)) {
                    go = Some(false);
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(PAD as i8, 10))
                    .show(ui, |ui| {
                        ui.set_width(W - 2.0 * PAD);
                        ui.spacing_mut().item_spacing.y = 8.0;
                        preview(ui, &t.read.table);
                        if t.format != table_io::Format::Json
                            && ui::switch_row(
                                ui,
                                "First row is the header",
                                t.header,
                                ui::CONTROL_H,
                            )
                            .clicked()
                        {
                            t.header = !t.header;
                            reread = true;
                        }
                    });
                if !t.read.notes.is_empty() {
                    ui::card_rule(ui);
                    note_row(ui, &t.read.notes.join(" · "));
                }
                ui::card_rule(ui);
                let buttons = [
                    ("Cancel", FieldButton::Off),
                    ("Replace data", FieldButton::Primary),
                ];
                let (linkable, link) = (t.linkable, &mut t.link);
                let lead = |ui: &mut egui::Ui, row: egui::Rect| {
                    if linkable {
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(row.with_max_x(row.center().x))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        );
                        ui::tick_box(&mut child, "Keep this file linked", "", link);
                    }
                };
                match crate::settings::card_foot_with(ui, W, &buttons, None, lead) {
                    Some(0) => go = Some(false),
                    Some(_) => go = Some(true),
                    None => {}
                }
            });
        if clicked_away || go == Some(false) {
            return;
        }
        if reread && let Ok(r) = table_io::read(&t.text, t.format, Some(t.header)) {
            t.read = r;
        }
        if go == Some(true) || (enter && go.is_none()) {
            let link = (t.link && t.linkable).then(|| {
                let (modified, size) = stat_of(&t.path).unwrap_or_default();
                DataLink {
                    path: t.path.to_string_lossy().into_owned(),
                    size,
                    modified,
                }
            });
            let rows = t.read.table.rows.len();
            let name = file_name(&t.path.to_string_lossy());
            if self.replace_chart_table(t.chart, t.read.table.clone(), link, "Import chart data") {
                let series = self
                    .session
                    .doc
                    .get(t.chart)
                    .and_then(|n| n.chart())
                    .map_or(0, |c| c.mapping.series.len());
                self.session.info(format!(
                    "Imported {name} \u{b7} {}, {}",
                    ui::count_as(rows, "row", "rows"),
                    ui::count_as(series, "series", "series")
                ));
            }
            return;
        }
        self.chart_import = Some(t);
    }

    /// The linked files' state, `stat`ed every `kits::updates::WATCH_EVERY` and
    /// when the window regains focus, as the tokens link is.
    pub(crate) fn watch_chart_links(&mut self, ctx: &egui::Context) {
        let (now, focused) = ctx.input(|i| (i.time, i.viewport().focused.unwrap_or(true)));
        let regained = focused && !self.chart_watch.focused;
        self.chart_watch.focused = focused;
        let mut paths: Vec<String> = charts(&self.session.doc)
            .into_iter()
            .filter_map(|(_, c)| c.source.as_ref().map(|s| s.path.clone()))
            .collect();
        paths.sort();
        paths.dedup();
        let known: Vec<&String> = self.chart_watch.seen.iter().map(|(p, _)| p).collect();
        let new_path = paths.iter().any(|p| !known.contains(&p)) || known.len() != paths.len();
        if paths.is_empty() {
            self.chart_watch.seen.clear();
            return;
        }
        if now < self.chart_watch.next && !regained && !new_path {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                self.chart_watch.next - now,
            ));
            return;
        }
        self.chart_watch.next = now + crate::kits::updates::WATCH_EVERY;
        self.chart_watch.seen = paths
            .into_iter()
            .map(|p| {
                let s = stat_of(Path::new(&p));
                (p, s)
            })
            .collect();
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            crate::kits::updates::WATCH_EVERY,
        ));
    }

    /// Chart `id`'s link as the watch last saw it; `None` when it is not linked.
    pub(crate) fn chart_link_state(&self, id: NodeId) -> Option<LinkState> {
        let link = self.session.doc.get(id)?.chart()?.source.as_ref()?;
        let seen = self.chart_watch.seen.iter().find(|(p, _)| *p == link.path);
        Some(match seen {
            None => LinkState::Current,
            Some((_, None)) => LinkState::Missing,
            Some((_, Some((m, s)))) if *m == link.modified && *s == link.size => LinkState::Current,
            Some(_) => LinkState::Changed,
        })
    }

    /// The linked charts whose file changed, by path, with their count.
    fn changed_links(&self) -> Vec<(String, Vec<NodeId>)> {
        let mut out: Vec<(String, Vec<NodeId>)> = Vec::new();
        let ids: Vec<NodeId> = charts(&self.session.doc)
            .into_iter()
            .filter(|(_, c)| c.source.is_some())
            .map(|(id, _)| id)
            .collect();
        for id in ids {
            if self.chart_link_state(id) != Some(LinkState::Changed) {
                continue;
            }
            let path = self
                .session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .and_then(|c| c.source.as_ref())
                .map(|s| s.path.clone());
            let Some(path) = path else { continue };
            match out.iter_mut().find(|(p, _)| *p == path) {
                Some((_, list)) => list.push(id),
                None => out.push((path, vec![id])),
            }
        }
        out
    }

    /// *Update chart data*: read `path` and replace every chart linked to it, one
    /// undo step. The file's contents enter the document only here, on a click.
    pub(crate) fn update_linked_charts(&mut self, path: &str) {
        let name = file_name(path);
        let ids: Vec<NodeId> = self
            .changed_links()
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, ids)| ids)
            .unwrap_or_default();
        let Some(format) = table_io::Format::of_name(&name) else {
            return;
        };
        let Some(text) =
            crate::tokens_import::read_linked(Path::new(path), crate::tokens_import::MAX_READ)
        else {
            return self.session.fail(format!("Cannot read {name}"));
        };
        let read = match table_io::read(&text, format, None) {
            Ok(r) => r,
            Err(e) => return self.session.fail(format!("Cannot update from {name}: {e}")),
        };
        let (modified, size) = stat_of(Path::new(path)).unwrap_or_default();
        let link = DataLink {
            path: path.to_string(),
            size,
            modified,
        };
        let ops: Vec<Operation> = ids
            .iter()
            .filter_map(|id| {
                let chart = self.session.doc.get(*id)?.chart()?;
                let next = Chart {
                    source: Some(link.clone()),
                    ..chart.with_table(read.table.clone())
                };
                Some(Operation::SetChart {
                    id: *id,
                    chart: Some(Box::new(next)),
                })
            })
            .collect();
        let n = ops.len();
        if n == 0 {
            return;
        }
        self.session.name_step("Update chart data", icon::IMPORT);
        if self.commit_edit(Transaction(ops)) {
            self.session.info(match n {
                1 => format!("Updated 1 chart from {name}"),
                n => format!("Updated {n} charts from {name}"),
            });
        }
    }

    /// *Unlink data file*: chart `id` keeps its table and drops its link.
    pub(crate) fn unlink_chart_data(&mut self, id: NodeId) {
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()) else {
            return;
        };
        if chart.source.is_none() {
            return;
        }
        let next = Chart {
            source: None,
            ..chart.clone()
        };
        self.session
            .name_step("Unlink data file", icon::LINK_SIMPLE);
        self.commit_edit(Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(next)),
        }]));
    }

    /// The pill beside the tokens pill: *sales.csv changed · Update charts*, the
    /// count when several charts link it. A click updates them.
    pub(crate) fn chart_link_pill(&mut self, ui: &mut egui::Ui) {
        let Some((path, ids)) = self.changed_links().into_iter().next() else {
            return;
        };
        let name = file_name(&path);
        let text = match ids.len() {
            1 => format!("{name} changed \u{b7} Update chart"),
            n => format!("{name} changed \u{b7} Update {n} charts"),
        };
        let scale = ui::Scale::Toolbar;
        let glyph_pt = scale.glyph_pt();
        let font = egui::FontId::proportional(scale.text_pt());
        let text_w = ui
            .ctx()
            .fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), color::text()))
            .size()
            .x;
        let w = 9.0 + glyph_pt + 6.0 + text_w + 9.0;
        let (rect, resp, fg) = ui::button_face(ui, egui::vec2(w, ui::TOOLBAR_H), FieldButton::On);
        let p = ui.painter();
        let y = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 9.0, y),
            egui::Align2::LEFT_CENTER,
            icon::IMPORT,
            theme::icon_font(glyph_pt),
            fg,
        );
        p.text(
            egui::pos2(rect.left() + 9.0 + glyph_pt + 6.0, y),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            fg,
        );
        if resp
            .on_hover_text(format!(
                "{name} changed on disk \u{b7} click to read it into its charts"
            ))
            .clicked()
        {
            self.update_linked_charts(&path);
        }
    }

    /// The Chart card's source line for chart `id`: the file, its state, and
    /// *Update*, *Relink…* and *Unlink* as they apply.
    pub(crate) fn chart_source_line(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(state) = self.chart_link_state(id) else {
            return;
        };
        let Some(path) = self
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .and_then(|c| c.source.as_ref())
            .map(|s| s.path.clone())
        else {
            return;
        };
        let name = file_name(&path);
        let (said, ink) = match state {
            LinkState::Current => (format!("Linked to {name}"), theme::text::muted()),
            LinkState::Changed => (format!("{name} · Changed on disk"), color::accent_300()),
            LinkState::Missing => (format!("{name} · File not found"), color::warn()),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(theme::icon_text(
                icon::LINK_SIMPLE,
                13.0,
                theme::text::dim(),
            ));
            ui.add(
                egui::Label::new(egui::RichText::new(said).size(ui::SMALL_PT).color(ink))
                    .truncate(),
            )
            .on_hover_text(&path);
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            if state == LinkState::Changed && ui::word_link(ui, "Update", false).clicked() {
                self.update_linked_charts(&path);
            }
            if state == LinkState::Missing && ui::word_link(ui, "Relink…", false).clicked() {
                self.import_chart_data_dialog(id);
            }
            if ui::word_link(ui, "Unlink", true).clicked() {
                self.unlink_chart_data(id);
            }
        });
    }

    /// A lone table file dropped on the window: onto a chart it opens the
    /// import box for that chart; onto bare canvas a `.csv` or `.tsv` makes a
    /// chart there from the file. Answers whether it took the drop.
    pub(crate) fn drop_chart_data(
        &mut self,
        file: &egui::DroppedFile,
        name: &str,
        at: Option<ondin_core::kurbo::Point>,
    ) -> bool {
        let Some(format) = table_io::Format::of_name(name) else {
            return false;
        };
        let target = at.and_then(|p| {
            ondin_core::hit_test(
                &self.session.resolved,
                &self.session.doc,
                self.session.page,
                p,
                0.0,
            )
            .into_iter()
            .find(|id| {
                self.session
                    .doc
                    .get(*id)
                    .is_some_and(|n| n.chart().is_some())
            })
        });
        let path = file.path.clone().unwrap_or_else(|| PathBuf::from(name));
        if let Some(id) = target {
            self.session.selection.set_one(id);
            self.open_chart_import(
                id,
                path,
                file.path
                    .is_none()
                    .then(|| file.bytes.as_deref().unwrap_or_default().to_vec()),
            );
            return true;
        }
        if format == table_io::Format::Json {
            return false;
        }
        let text = match (&file.path, &file.bytes) {
            (Some(p), _) => crate::tokens_import::read_linked(p, crate::tokens_import::MAX_READ),
            (None, Some(b)) => Some(String::from_utf8_lossy(b).into_owned()),
            (None, None) => None,
        };
        let Some(text) = text else {
            self.session.fail(format!(
                "Cannot import {name}: not a local file of at most 16 MB"
            ));
            return true;
        };
        let read = match table_io::read(&text, format, None) {
            Ok(r) => r,
            Err(e) => {
                self.session.fail(format!("Cannot import {name}: {e}"));
                return true;
            }
        };
        let kind = kind_for(&read.table);
        let mut c = Chart::sample(kind);
        c.mapping = chart::Mapping::guess(kind, &read.table);
        c.table = read.table;
        c.source = file.path.as_ref().map(|p| {
            let (modified, size) = stat_of(p).unwrap_or_default();
            DataLink {
                path: p.to_string_lossy().into_owned(),
                size,
                modified,
            }
        });
        let p = at.unwrap_or_else(|| self.viewport_centre());
        self.insert_chart_at(p, c);
        true
    }
}

/// The first rows of `table` as they will land, each column's type tag over it.
fn preview(ui: &mut egui::Ui, table: &chart::Table) {
    let rows = table.rows.len().min(PREVIEW_ROWS);
    let w = PREVIEW_COL_W * table.columns.len() as f32;
    let h = PREVIEW_ROW_H * (rows as f32 + 1.0);
    egui::ScrollArea::horizontal()
        .id_salt("chart-import-preview")
        .max_height(h + 8.0)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(w.max(ui.available_width()), h),
                egui::Sense::hover(),
            );
            let p = ui.painter();
            let hairline = egui::Stroke::new(1.0, theme::color::field_border());
            for (c, column) in table.columns.iter().enumerate() {
                let x = rect.left() + PREVIEW_COL_W * c as f32;
                let numeric = table.is_numeric(column.id);
                let head = egui::Rect::from_min_size(
                    egui::pos2(x, rect.top()),
                    egui::vec2(PREVIEW_COL_W, PREVIEW_ROW_H),
                );
                let clip = p.with_clip_rect(head.shrink(1.0));
                let tag = if numeric { "#" } else { "Aa" };
                clip.text(
                    egui::pos2(head.left() + 6.0, head.center().y),
                    egui::Align2::LEFT_CENTER,
                    format!("{tag}  {}", column.name),
                    egui::FontId::proportional(ui::SMALL_PT),
                    theme::text::strong(),
                );
                for r in 0..rows {
                    let cell = egui::Rect::from_min_size(
                        egui::pos2(x, rect.top() + PREVIEW_ROW_H * (r as f32 + 1.0)),
                        egui::vec2(PREVIEW_COL_W, PREVIEW_ROW_H),
                    );
                    p.rect_filled(cell, 0.0, theme::color::field());
                    p.rect_stroke(cell, 0.0, hairline, egui::StrokeKind::Inside);
                    let text = &table.rows[r][c];
                    let clip = p.with_clip_rect(cell.shrink(1.0));
                    let (pos, align) = if numeric {
                        (
                            egui::pos2(cell.right() - 6.0, cell.center().y),
                            egui::Align2::RIGHT_CENTER,
                        )
                    } else {
                        (
                            egui::pos2(cell.left() + 6.0, cell.center().y),
                            egui::Align2::LEFT_CENTER,
                        )
                    };
                    clip.text(
                        pos,
                        align,
                        text,
                        egui::FontId::proportional(ui::SMALL_PT),
                        color::text(),
                    );
                }
            }
        });
    if table.rows.len() > PREVIEW_ROWS {
        ui.label(
            egui::RichText::new(format!(
                "and {} more rows",
                chart::grouped(table.rows.len() - PREVIEW_ROWS)
            ))
            .size(ui::SMALL_PT)
            .color(theme::text::faint()),
        );
    }
}

fn note_row(ui: &mut egui::Ui, note: &str) {
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(PAD as i8, 9))
        .show(ui, |ui| {
            ui.set_width(W - 2.0 * PAD);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.label(theme::icon_text(icon::INFO, 13.0, theme::text::dim()));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(note)
                            .size(ui::SMALL_PT)
                            .color(theme::text::dim()),
                    )
                    .wrap(),
                );
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dropped table's kind follows its columns.
    #[test]
    fn a_dropped_table_picks_its_kind_from_its_columns() {
        let read = |text: &str| {
            table_io::read(text, table_io::Format::Csv, None)
                .unwrap()
                .table
        };
        assert_eq!(kind_for(&read("x,y\n1,2\n")), ChartKind::Scatter);
        assert_eq!(kind_for(&read("from,to,v\na,b,3\n")), ChartKind::Sankey);
        assert_eq!(kind_for(&read("month,a,b\nJan,1,2\n")), ChartKind::Column);
    }

    fn temp_csv(tag: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-chart-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sales.csv");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn pass(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    fn enter() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Importing a CSV with *Keep this file linked* replaces the table in one undo
    /// step, keeps the series whose column names survive and links the file; a
    /// change on disk is seen by the watch, and *Update* reads it in.
    #[test]
    fn an_imported_file_is_linked_and_updates_when_it_changes() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let page = app.session.doc.first_page();
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().expect("the chart");
        assert_eq!(app.session.doc.get(id).and_then(|n| n.parent()), Some(page));
        let revenue = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .mapping
            .series[0];

        let path = temp_csv("link", "Category,Revenue,Units\nQ1,10,3\nQ2,12,4\n");
        app.open_chart_import(id, path.clone(), None);
        assert!(app.chart_import.is_some(), "the box opened");
        pass(&ctx, &mut app, Vec::new());
        pass(&ctx, &mut app, vec![enter()]);
        assert!(app.chart_import.is_none(), "Enter replaced the data");
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(chart.table.rows.len(), 2);
        assert!(
            chart.mapping.series.contains(&revenue),
            "Revenue kept its column and its place"
        );
        let link = chart.source.clone().expect("linked");
        assert_eq!(Path::new(&link.path), path.as_path());

        app.watch_chart_links(&ctx);
        assert_eq!(app.chart_link_state(id), Some(LinkState::Current));
        std::fs::write(&path, "Category,Revenue,Units\nQ1,10,3\nQ2,12,4\nQ3,15,6\n").unwrap();
        app.chart_watch.next = 0.0;
        app.watch_chart_links(&ctx);
        assert_eq!(app.chart_link_state(id), Some(LinkState::Changed));
        app.update_linked_charts(&link.path);
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).unwrap();
        assert_eq!(chart.table.rows.len(), 3, "the new row read in");
        app.chart_watch.next = 0.0;
        app.watch_chart_links(&ctx);
        assert_eq!(app.chart_link_state(id), Some(LinkState::Current));
        app.session.undo();
        assert_eq!(
            app.session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .unwrap()
                .table
                .rows
                .len(),
            2,
            "Update is one undo step"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A file a document's link names is read only when it is local.
    #[test]
    fn a_link_is_refused_off_a_local_drive() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().unwrap();
        app.open_chart_import(id, PathBuf::from(r"\\host\share\sales.csv"), None);
        assert!(app.chart_import.is_none(), "a share is never read");
    }
}
