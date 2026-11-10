//! The chart data window (`charts.md`, *The data window*): a floating card over
//! the bottom of the canvas column holding the selected chart's table as a
//! grid. Non-modal: each committed cell, paste, clear, insert or delete is one
//! `SetChart` (*Edit chart data*), so the chart redraws as the table changes.
//!
//! The grid is one focusable region that locks the arrows, `Tab` and `Escape`
//! while it has the keyboard, so the canvas keymap stays quiet; a cell being
//! typed into is a `ui::text_field` laid over the cell.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use ondin_core::chart::{self, Chart, ChartKind, ColumnId};
use ondin_core::{NodeId, Operation, Transaction};

/// The card's size; narrower when the canvas column is ([`OndinApp::chart_data_ui`]).
pub(crate) const W: f32 = 640.0;
const H: f32 = 320.0;
const PAD: f32 = 12.0;
/// The card's gap from the canvas column's bottom.
const MARGIN: f32 = 16.0;
/// A cell's width and height, the row-number gutter's width.
const COL_W: f32 = 112.0;
const ROW_H: f32 = ui::MENU_ROW_H;
const GUTTER: f32 = 40.0;
/// The space a cell's text keeps from its edges.
const CELL_PAD: f32 = 6.0;
/// The grid's focus id.
const GRID_ID: &str = "chart-data-grid";
/// The undo step every edit makes.
const STEP: &str = "Edit chart data";

/// A cell by row and column index in the table.
pub(crate) type Cell = (usize, usize);

/// The window while it is open.
#[derive(Clone, Debug)]
pub(crate) struct ChartData {
    /// The chart it shows; selecting anything else closes it.
    pub(crate) chart: NodeId,
    /// The cursor, and the other corner of the selected range.
    pub(crate) at: Cell,
    pub(crate) anchor: Cell,
    /// A cell being typed into: its text, kept until committed or abandoned.
    edit: Option<String>,
    /// A header being renamed: its column and text.
    rename: Option<(usize, String)>,
    /// The open header or row menu.
    menu: Option<Menu>,
    /// Ask for the grid's focus on the next pass.
    focus: bool,
    /// Scroll the cursor into view on the next pass.
    reveal: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Menu {
    Header(usize, egui::Pos2),
    Row(usize, egui::Pos2),
}

impl ChartData {
    fn new(chart: NodeId, at: Cell) -> Self {
        Self {
            chart,
            at,
            anchor: at,
            edit: None,
            rename: None,
            menu: None,
            focus: true,
            reveal: true,
        }
    }

    /// The selected range, rows then columns, inclusive.
    fn range(
        &self,
    ) -> (
        std::ops::RangeInclusive<usize>,
        std::ops::RangeInclusive<usize>,
    ) {
        let (r0, r1) = (self.at.0.min(self.anchor.0), self.at.0.max(self.anchor.0));
        let (c0, c1) = (self.at.1.min(self.anchor.1), self.at.1.max(self.anchor.1));
        (r0..=r1, c0..=c1)
    }
}

/// What a column is to the chart: its header's role mark.
enum Role {
    Category,
    Series(Option<egui::Color32>),
    /// A Sankey's From, To or Value.
    Named(&'static str),
    Unmapped,
}

fn role_of(chart: &Chart, col: ColumnId) -> Role {
    let m = &chart.mapping;
    if chart.kind == ChartKind::Sankey {
        return match col {
            c if Some(c) == m.from => Role::Named("From"),
            c if Some(c) == m.to => Role::Named("To"),
            c if Some(c) == m.value => Role::Named("Value"),
            _ => Role::Unmapped,
        };
    }
    if Some(col) == m.category {
        return Role::Category;
    }
    if m.series.contains(&col) {
        let colour = crate::panels::chart_colour(chart, col);
        return Role::Series(colour.map(ui::color_to_egui));
    }
    Role::Unmapped
}

/// The cell behind a double-clicked part: a mark's value (its series' column
/// at its row), or a label's or a legend's column at the first row.
pub(crate) fn cell_of(chart: &Chart, key: Option<chart::PartKey>) -> Cell {
    use chart::PartRole as R;
    let t = &chart.table;
    let Some(key) = key else { return (0, 0) };
    let col_of = |id: u32| t.index_of(ColumnId(id));
    let at = |c: Option<ColumnId>| c.and_then(|c| t.index_of(c));
    let last = t.rows.len().saturating_sub(1);
    let row = (key.index as usize).min(last);
    let first_series = at(chart.mapping.series.first().copied());
    let cell = match key.role {
        // A mark's value: its series' column at its row.
        R::Bar | R::Point | R::ValueLabel => col_of(key.series).map(|c| (row, c)),
        // A whole line or area, and a legend entry: its column's first row.
        R::Line | R::Area | R::LegendItem | R::LegendSwatch | R::LegendLabel => {
            col_of(key.series).map(|c| (0, c))
        }
        // A slice's value is the first series' at its category's row.
        R::Slice => first_series.map(|c| (row, c)),
        R::CategoryLabel => at(chart.mapping.category).map(|c| (row, c)),
        // A flow is its row; a node is the first row naming it.
        R::Flow => at(chart.mapping.value).map(|c| (row, c)),
        R::SankeyNode | R::NodeLabel => {
            let name = chart::sankey_nodes(chart)
                .into_iter()
                .nth(key.index as usize)
                .map(|(n, _)| n);
            let (from, to) = (at(chart.mapping.from), at(chart.mapping.to));
            name.and_then(|n| {
                t.rows.iter().enumerate().find_map(|(r, cells)| {
                    [from, to]
                        .into_iter()
                        .flatten()
                        .find(|c| cells[*c].trim() == n)
                        .map(|c| (r, c))
                })
            })
        }
        _ => None,
    };
    cell.unwrap_or((0, 0))
}

// --- table edits, pure, so a test reads them without a window ------------------

/// `table` with `cells` written from `at` down and right, growing the table to
/// fit (a paste from a spreadsheet); held to the table's limits.
pub(crate) fn pasted(table: &chart::Table, at: Cell, cells: &[Vec<String>]) -> chart::Table {
    let mut t = table.clone();
    let width = cells.iter().map(Vec::len).max().unwrap_or(0);
    while t.columns.len() < (at.1 + width).min(chart::MAX_COLUMNS) {
        let end = t.columns.len();
        add_column(&mut t, end);
    }
    let rows = (at.0 + cells.len()).min(chart::MAX_ROWS);
    while t.rows.len() < rows {
        t.rows.push(vec![String::new(); t.columns.len()]);
    }
    for (dr, line) in cells.iter().enumerate() {
        for (dc, cell) in line.iter().enumerate() {
            let (r, c) = (at.0 + dr, at.1 + dc);
            if r < t.rows.len() && c < t.columns.len() {
                t.rows[r][c] = cell.chars().take(chart::MAX_CELL_CHARS).collect();
            }
        }
    }
    t
}

/// Tab-separated text read as rows of cells, as a spreadsheet copies them.
pub(crate) fn tsv_cells(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    text.split('\n')
        .map(|l| {
            l.strip_suffix('\r')
                .unwrap_or(l)
                .split('\t')
                .map(str::to_string)
                .collect()
        })
        .collect()
}

/// The range's cells as tab-separated text.
fn tsv_of(
    table: &chart::Table,
    rows: std::ops::RangeInclusive<usize>,
    cols: std::ops::RangeInclusive<usize>,
) -> String {
    rows.filter_map(|r| table.rows.get(r))
        .map(|row| {
            cols.clone()
                .filter_map(|c| row.get(c).map(String::as_str))
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A new empty column at `at`, named *Column N* with the first free N.
fn add_column(t: &mut chart::Table, at: usize) {
    let id = t.next_column_id();
    let mut n = t.columns.len() + 1;
    while t.columns.iter().any(|c| c.name == format!("Column {n}")) {
        n += 1;
    }
    t.columns.insert(
        at.min(t.columns.len()),
        chart::Column {
            id,
            name: format!("Column {n}"),
        },
    );
    for row in &mut t.rows {
        row.insert(at.min(row.len()), String::new());
    }
}

/// `chart` with column `col` gone, and gone from the mapping and the series.
fn without_column(chart: &Chart, col: usize) -> Chart {
    let mut next = chart.clone();
    let Some(id) = next.table.columns.get(col).map(|c| c.id) else {
        return next;
    };
    next.table.columns.remove(col);
    for row in &mut next.table.rows {
        if col < row.len() {
            row.remove(col);
        }
    }
    let m = &mut next.mapping;
    for slot in [
        &mut m.category,
        &mut m.size,
        &mut m.group,
        &mut m.from,
        &mut m.to,
        &mut m.value,
    ] {
        if *slot == Some(id) {
            *slot = None;
        }
    }
    m.series.retain(|c| *c != id);
    next.series.retain(|s| s.column != id);
    next
}

/// `table` sorted by column `col`, numbers by value when the column is a
/// number column, text otherwise; empty cells last either way.
fn sorted(table: &chart::Table, col: usize, ascending: bool) -> chart::Table {
    let mut t = table.clone();
    let numeric = t.columns.get(col).is_some_and(|c| table.is_numeric(c.id));
    t.rows.sort_by(|a, b| {
        let (x, y) = (a[col].trim(), b[col].trim());
        match (x.is_empty(), y.is_empty()) {
            (true, true) => return std::cmp::Ordering::Equal,
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            _ => {}
        }
        let o = if numeric {
            let (p, q) = (chart::parse_number(x), chart::parse_number(y));
            p.partial_cmp(&q).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            x.to_lowercase().cmp(&y.to_lowercase())
        };
        if ascending { o } else { o.reverse() }
    });
    t
}

impl OndinApp {
    /// Open the window on chart `id` with `at` selected, or move it there.
    pub(crate) fn open_chart_data(&mut self, id: NodeId, at: Cell) {
        if self.session.doc.get(id).and_then(|n| n.chart()).is_none() {
            return;
        }
        self.chart_data = Some(ChartData::new(id, at));
    }

    /// Close the window, dropping a cell being typed into.
    pub(crate) fn close_chart_data(&mut self) {
        self.chart_data = None;
    }

    /// Write `table` as chart `id`'s, one undo step.
    fn commit_table(&mut self, id: NodeId, chart: &Chart, table: chart::Table) {
        if table == chart.table {
            return;
        }
        self.commit_chart(
            id,
            Chart {
                table,
                ..chart.clone()
            },
        );
    }

    fn commit_chart(&mut self, id: NodeId, next: Chart) {
        self.session.name_step(STEP, icon::TABLE);
        self.commit_edit(Transaction(vec![Operation::SetChart {
            id,
            chart: Some(Box::new(next)),
        }]));
    }

    /// The window, each frame: closed when its chart is no longer the one
    /// selected chart.
    pub(crate) fn chart_data_ui(&mut self, ctx: &egui::Context) {
        let Some(open) = &mut self.chart_data else {
            return;
        };
        // The import box is modal over it: the grid lets go of the keyboard while
        // it is up, and takes it back after.
        if self.chart_import.is_some() {
            let grid = egui::Id::new(GRID_ID);
            ctx.memory_mut(|m| m.surrender_focus(grid));
            open.focus = true;
            return;
        }
        let id = open.chart;
        let still = matches!(self.session.selection.ids(), [one] if *one == id);
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()).cloned() else {
            self.chart_data = None;
            return;
        };
        if !still {
            // Selecting another chart moves the window there.
            match self.session.selection.ids() {
                [other]
                    if self
                        .session
                        .doc
                        .get(*other)
                        .is_some_and(|n| n.chart().is_some()) =>
                {
                    self.chart_data = Some(ChartData::new(*other, (0, 0)));
                }
                _ => self.chart_data = None,
            }
            return;
        }
        let name = self
            .session
            .doc
            .get(id)
            .map(|n| n.name().to_string())
            .unwrap_or_default();
        let column = self.canvas_rect;
        let w = W.min(column.width() - 32.0).max(320.0);
        let anchor = egui::pos2(column.center().x, column.bottom() - MARGIN);
        let linked = chart.source.is_some();
        let mut act: Vec<Act> = Vec::new();
        let Some(mut state) = self.chart_data.take() else {
            return;
        };
        egui::Area::new(egui::Id::new("chart-data"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::CENTER_BOTTOM)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                ui::card_frame().show(ui, |ui| {
                    ui.set_width(w);
                    ui.set_height(H);
                    egui::Frame::NONE.inner_margin(PAD).show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        head(ui, &name, linked, &mut act);
                        if linked {
                            ui.label(
                                egui::RichText::new(linked_line(&chart))
                                    .size(ui::SMALL_PT)
                                    .color(theme::text::dim()),
                            );
                        }
                        grid(ui, &chart, &mut state, linked, &mut act);
                    });
                });
            });
        self.chart_data = Some(state);
        for a in act {
            match a {
                Act::Close => self.close_chart_data(),
                Act::Import => self.import_chart_data_dialog(id),
                Act::Table(t) => self.commit_table(id, &chart, t),
                Act::Chart(c) => self.commit_chart(id, *c),
                Act::Copy(text) => ctx.copy_text(text),
            }
        }
    }
}

/// What the window asks of the app after its pass.
enum Act {
    Close,
    Import,
    Table(chart::Table),
    Chart(Box<Chart>),
    Copy(String),
}

/// A word button at the head's height, as wide as Find's (`find::button_w`).
fn word_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let w = crate::find::button_w(ui, label);
    ui::label_button(ui, label, egui::vec2(w, ROW_H), ui::FieldButton::Off)
}

/// *Edited in sales.csv — unlink to edit here*.
fn linked_line(chart: &Chart) -> String {
    let file = chart
        .source
        .as_ref()
        .and_then(|s| {
            std::path::Path::new(&s.path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "a file".to_string());
    format!("Edited in {file} — unlink to edit here")
}

/// The head: the table glyph, *Chart data*, the chart's name, *Import…*, ✕;
/// the toolbar under it.
fn head(ui: &mut egui::Ui, name: &str, linked: bool, act: &mut Vec<Act>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(theme::icon_text(
            icon::TABLE,
            ui::CONTROL_GLYPH_PT,
            theme::text::muted(),
        ));
        ui.label(
            egui::RichText::new("Chart data")
                .size(ui::HEAD_PT)
                .color(theme::text::strong()),
        );
        ui.add(
            egui::Label::new(
                egui::RichText::new(name)
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
            )
            .truncate(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui::close_button(ui).on_hover_text("Close (Esc)").clicked() {
                act.push(Act::Close);
            }
            if word_button(ui, "Import…")
                .on_hover_text("Replace the table with a CSV, TSV or JSON file")
                .clicked()
            {
                act.push(Act::Import);
            }
        });
    });
    let _ = linked;
}

/// The grid: the header row and the row gutter held at the viewport's edges
/// over the scrolled cells.
fn grid(ui: &mut egui::Ui, chart: &Chart, s: &mut ChartData, linked: bool, act: &mut Vec<Act>) {
    let t = &chart.table;
    let (nr, nc) = (t.rows.len(), t.columns.len());
    if !linked {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if word_button(ui, "Add row").clicked() && nr < chart::MAX_ROWS {
                let mut next = t.clone();
                next.rows.push(vec![String::new(); nc]);
                s.at = (nr, s.at.1.min(nc.saturating_sub(1)));
                s.anchor = s.at;
                s.reveal = true;
                act.push(Act::Table(next));
            }
            if word_button(ui, "Add column").clicked() && nc < chart::MAX_COLUMNS {
                let mut next = t.clone();
                add_column(&mut next, nc);
                s.at = (s.at.0, nc);
                s.anchor = s.at;
                s.reveal = true;
                act.push(Act::Table(next));
            }
        });
    }
    // Keep the cursor inside the table as it shrinks under an undo.
    s.at = (
        s.at.0.min(nr.saturating_sub(1)),
        s.at.1.min(nc.saturating_sub(1)),
    );
    s.anchor = (
        s.anchor.0.min(nr.saturating_sub(1)),
        s.anchor.1.min(nc.saturating_sub(1)),
    );

    let grid_id = egui::Id::new(GRID_ID);
    let content = egui::vec2(GUTTER + COL_W * nc as f32, ROW_H * (nr as f32 + 1.0));
    let editing = s.edit.is_some() || s.rename.is_some();
    keys(ui, chart, s, linked, act, grid_id, editing);

    let scroll = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .id_salt("chart-data-scroll");
    scroll.show_viewport(ui, |ui, view| {
        let (rect, _) = ui.allocate_exact_size(content, egui::Sense::hover());
        let resp = ui.interact(rect, grid_id, egui::Sense::click_and_drag());
        if s.focus {
            resp.request_focus();
            s.focus = false;
        }
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    grid_id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        let origin = rect.min;
        let cell_rect = |r: usize, c: usize| {
            egui::Rect::from_min_size(
                origin + egui::vec2(GUTTER + COL_W * c as f32, ROW_H * (r as f32 + 1.0)),
                egui::vec2(COL_W, ROW_H),
            )
        };
        if s.reveal {
            ui.scroll_to_rect(
                cell_rect(s.at.0, s.at.1).expand2(egui::vec2(GUTTER, ROW_H)),
                None,
            );
            s.reveal = false;
        }
        // The pointer picks a cell; a drag extends the range.
        let at_pointer = |p: egui::Pos2| -> Option<(Option<usize>, Option<usize>)> {
            let local = p - origin;
            let col = ((local.x - GUTTER) / COL_W).floor();
            let row = (local.y / ROW_H).floor() - 1.0;
            let c = (local.x >= view.min.x + GUTTER - 0.0 && col >= 0.0 && (col as usize) < nc)
                .then_some(col as usize);
            let r = (local.y >= view.min.y + ROW_H && row >= 0.0 && (row as usize) < nr)
                .then_some(row as usize);
            Some((r, c))
        };
        let shift = ui.input(|i| i.modifiers.shift);
        if (resp.clicked() || resp.drag_started() || resp.secondary_clicked())
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((r, c)) = at_pointer(p)
        {
            resp.request_focus();
            match (r, c) {
                (Some(r), Some(c)) => {
                    commit_edit_text(chart, s, linked, act);
                    s.at = (r, c);
                    if !shift {
                        s.anchor = s.at;
                    }
                }
                (None, Some(c)) if resp.secondary_clicked() => s.menu = Some(Menu::Header(c, p)),
                (Some(r), None) if resp.secondary_clicked() => s.menu = Some(Menu::Row(r, p)),
                (None, Some(c)) => {
                    // A header click selects its column.
                    s.anchor = (0, c);
                    s.at = (nr.saturating_sub(1), c);
                }
                _ => {}
            }
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((Some(r), Some(c))) = at_pointer(p)
        {
            s.at = (r, c);
        }
        if resp.double_clicked()
            && let Some(p) = resp.interact_pointer_pos()
            && let Some((r, c)) = at_pointer(p)
        {
            match (r, c) {
                (Some(_), Some(_)) if !linked => s.edit = Some(t.rows[s.at.0][s.at.1].clone()),
                (None, Some(c)) => s.rename = Some((c, t.columns[c].name.clone())),
                _ => {}
            }
        }

        let p = ui.painter_at(view.translate(origin.to_vec2()));
        let first = ((view.min.y / ROW_H).floor() as usize).saturating_sub(1);
        let last = (((view.max.y / ROW_H).ceil() as usize) + 1).min(nr);
        let c_first = (((view.min.x - GUTTER).max(0.0) / COL_W).floor() as usize).min(nc);
        let c_last = ((((view.max.x - GUTTER).max(0.0) / COL_W).ceil() as usize) + 1).min(nc);
        let (rows_sel, cols_sel) = s.range();
        let hairline = egui::Stroke::new(1.0, theme::color::field_border());
        let font = egui::FontId::proportional(ui::CONTROL_PT);
        let numeric: Vec<bool> = t.columns.iter().map(|c| t.is_numeric(c.id)).collect();
        for r in first..last {
            for c in c_first..c_last {
                let rc = cell_rect(r, c);
                let lit = rows_sel.contains(&r) && cols_sel.contains(&c);
                p.rect_filled(
                    rc,
                    0.0,
                    if lit {
                        color::accent_a(36)
                    } else {
                        theme::color::field()
                    },
                );
                p.rect_stroke(rc, 0.0, hairline, egui::StrokeKind::Inside);
                let text = &t.rows[r][c];
                if !text.is_empty() && !(s.edit.is_some() && s.at == (r, c)) {
                    let ink = if linked {
                        theme::text::muted()
                    } else {
                        color::text()
                    };
                    let galley = p.layout_no_wrap(text.clone(), font.clone(), ink);
                    let x = if numeric.get(c) == Some(&true) && chart::parse_number(text).is_some()
                    {
                        rc.right() - CELL_PAD - galley.size().x
                    } else {
                        rc.left() + CELL_PAD
                    };
                    let clip = p.with_clip_rect(rc.shrink(1.0).intersect(p.clip_rect()));
                    clip.galley(
                        egui::pos2(x, rc.center().y - galley.size().y / 2.0),
                        galley,
                        ink,
                    );
                }
            }
        }
        // The cursor.
        if nr > 0 && nc > 0 {
            p.rect_stroke(
                cell_rect(s.at.0, s.at.1),
                0.0,
                egui::Stroke::new(1.5, color::accent()),
                egui::StrokeKind::Inside,
            );
        }
        // The gutter, held at the view's left.
        let gx = origin.x + view.min.x;
        for r in first..last {
            let rc = egui::Rect::from_min_size(
                egui::pos2(gx, cell_rect(r, 0).top()),
                egui::vec2(GUTTER, ROW_H),
            );
            p.rect_filled(rc, 0.0, theme::color::card());
            p.text(
                rc.center(),
                egui::Align2::CENTER_CENTER,
                (r + 1).to_string(),
                egui::FontId::proportional(ui::SMALL_PT),
                if rows_sel.contains(&r) {
                    theme::text::muted()
                } else {
                    theme::text::faint()
                },
            );
        }
        // The header, held at the view's top.
        let hy = origin.y + view.min.y;
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(gx, hy), egui::vec2(view.width(), ROW_H)),
            0.0,
            theme::color::card(),
        );
        for (c, column) in t.columns.iter().enumerate().take(c_last).skip(c_first) {
            let hr = egui::Rect::from_min_size(
                egui::pos2(cell_rect(0, c).left(), hy),
                egui::vec2(COL_W, ROW_H),
            );
            if hr.right() < gx + GUTTER {
                continue;
            }
            let clip = p.with_clip_rect(
                hr.intersect(egui::Rect::everything_right_of(gx + GUTTER))
                    .intersect(p.clip_rect()),
            );
            let tag = if numeric[c] { "#" } else { "Aa" };
            let mut x = hr.left() + CELL_PAD;
            let tag_g = clip.layout_no_wrap(
                tag.into(),
                egui::FontId::monospace(ui::SMALL_PT - 1.0),
                theme::text::faint(),
            );
            clip.galley(
                egui::pos2(x, hr.center().y - tag_g.size().y / 2.0),
                tag_g.clone(),
                theme::text::faint(),
            );
            x += tag_g.size().x + 5.0;
            let (ink, mark) = match role_of(chart, column.id) {
                Role::Category => (color::text(), Some("Category")),
                Role::Named(n) => (color::text(), Some(n)),
                Role::Series(colour) => {
                    if let Some(c) = colour {
                        let sw = egui::Rect::from_center_size(
                            egui::pos2(x + 4.0, hr.center().y),
                            egui::vec2(8.0, 8.0),
                        );
                        clip.rect_filled(sw, 2.0, c);
                        x += 13.0;
                    }
                    (color::text(), None)
                }
                Role::Unmapped => (theme::text::dim(), None),
            };
            let name_g = clip.layout_no_wrap(
                column.name.clone(),
                egui::FontId::proportional(ui::SMALL_PT),
                ink,
            );
            clip.galley(
                egui::pos2(x, hr.center().y - name_g.size().y / 2.0),
                name_g,
                ink,
            );
            if let Some(m) = mark {
                clip.text(
                    egui::pos2(hr.right() - CELL_PAD, hr.center().y),
                    egui::Align2::RIGHT_CENTER,
                    m,
                    egui::FontId::proportional(ui::SMALL_PT - 1.5),
                    theme::text::faint(),
                );
            }
        }
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(gx, hy), egui::vec2(GUTTER, ROW_H)),
            0.0,
            theme::color::card(),
        );

        // The cell being typed into, over its cell.
        if let Some(text) = &mut s.edit {
            let rc = cell_rect(s.at.0, s.at.1);
            let id = egui::Id::new("chart-data-cell");
            let resp = ui
                .scope_builder(egui::UiBuilder::new().max_rect(rc), |ui| {
                    ui::text_field(ui, rc.size(), text, "")
                })
                .inner;
            if !resp.has_focus() && !ui.memory(|m| m.has_focus(resp.id)) {
                resp.request_focus();
            }
            let _ = id;
        }
        if let Some((c, text)) = &mut s.rename {
            let hr = egui::Rect::from_min_size(
                egui::pos2(cell_rect(0, *c).left(), hy),
                egui::vec2(COL_W, ROW_H),
            );
            let resp = ui
                .scope_builder(egui::UiBuilder::new().max_rect(hr), |ui| {
                    ui::text_field(ui, hr.size(), text, "Column name")
                })
                .inner;
            if !resp.has_focus() {
                resp.request_focus();
            }
        }
    });

    if let Some(menu) = s.menu {
        menus(ui, chart, s, menu, linked, act);
    }
}

/// The grid's keys while it, or a cell in it, has the keyboard.
fn keys(
    ui: &mut egui::Ui,
    chart: &Chart,
    s: &mut ChartData,
    linked: bool,
    act: &mut Vec<Act>,
    grid_id: egui::Id,
    editing: bool,
) {
    let t = &chart.table;
    let (nr, nc) = (t.rows.len(), t.columns.len());
    let grid_focused = ui.memory(|m| m.has_focus(grid_id));
    let (events, shift, command) =
        ui.input(|i| (i.events.clone(), i.modifiers.shift, i.modifiers.command));
    let pressed = |k: egui::Key| {
        events
            .iter()
            .any(|e| matches!(e, egui::Event::Key { key, pressed: true, .. } if *key == k))
    };
    if editing {
        if let Some((c, name)) = &s.rename {
            if pressed(egui::Key::Escape) {
                s.rename = None;
                s.focus = true;
            } else if pressed(egui::Key::Enter) || pressed(egui::Key::Tab) {
                let name = name.trim();
                if !name.is_empty() && name != t.columns[*c].name {
                    let mut next = t.clone();
                    next.columns[*c].name = name.chars().take(chart::MAX_CELL_CHARS).collect();
                    act.push(Act::Table(next));
                }
                s.rename = None;
                s.focus = true;
            }
            return;
        }
        if pressed(egui::Key::Escape) {
            s.edit = None;
            s.focus = true;
            return;
        }
        let step = if pressed(egui::Key::Enter) {
            Some(if shift { (-1, 0) } else { (1, 0) })
        } else if pressed(egui::Key::Tab) {
            Some(if shift { (0, -1) } else { (0, 1) })
        } else {
            None
        };
        if let Some((dr, dc)) = step {
            commit_edit_text(chart, s, linked, act);
            s.at = moved(s.at, dr, dc, nr, nc);
            s.anchor = s.at;
            s.reveal = true;
            s.focus = true;
        }
        return;
    }
    if !grid_focused || nr == 0 || nc == 0 {
        if grid_focused && pressed(egui::Key::Escape) {
            act.push(Act::Close);
        }
        return;
    }
    let arrows = [
        (egui::Key::ArrowUp, (-1, 0)),
        (egui::Key::ArrowDown, (1, 0)),
        (egui::Key::ArrowLeft, (0, -1)),
        (egui::Key::ArrowRight, (0, 1)),
    ];
    for (k, (dr, dc)) in arrows {
        if pressed(k) {
            s.at = moved(s.at, dr, dc, nr, nc);
            if !shift {
                s.anchor = s.at;
            }
            s.reveal = true;
        }
    }
    if pressed(egui::Key::Tab) {
        s.at = moved(s.at, 0, if shift { -1 } else { 1 }, nr, nc);
        s.anchor = s.at;
    }
    if pressed(egui::Key::Escape) {
        act.push(Act::Close);
        return;
    }
    if linked {
        for e in &events {
            if matches!(e, egui::Event::Copy) {
                let (r, c) = s.range();
                act.push(Act::Copy(tsv_of(t, r, c)));
            }
        }
        return;
    }
    if pressed(egui::Key::F2) || pressed(egui::Key::Enter) {
        s.edit = Some(t.rows[s.at.0][s.at.1].clone());
        return;
    }
    if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
        let (rows, cols) = s.range();
        let mut next = t.clone();
        for r in rows {
            for c in cols.clone() {
                next.rows[r][c].clear();
            }
        }
        act.push(Act::Table(next));
        return;
    }
    for e in &events {
        match e {
            egui::Event::Text(text) if !command => {
                s.edit = Some(text.clone());
                return;
            }
            egui::Event::Copy => {
                let (r, c) = s.range();
                act.push(Act::Copy(tsv_of(t, r, c)));
            }
            egui::Event::Cut => {
                let (rows, cols) = s.range();
                act.push(Act::Copy(tsv_of(t, rows.clone(), cols.clone())));
                let mut next = t.clone();
                for r in rows {
                    for c in cols.clone() {
                        next.rows[r][c].clear();
                    }
                }
                act.push(Act::Table(next));
            }
            egui::Event::Paste(text) => {
                let cells = tsv_cells(text);
                let at = (*s.range().0.start(), *s.range().1.start());
                act.push(Act::Table(pasted(t, at, &cells)));
                let h = cells.len().max(1);
                let w = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
                s.anchor = at;
                s.at = (at.0 + h - 1, at.1 + w - 1);
            }
            _ => {}
        }
    }
}

/// `at` moved by a step, held inside the table.
fn moved(at: Cell, dr: isize, dc: isize, nr: usize, nc: usize) -> Cell {
    let r = (at.0 as isize + dr).clamp(0, nr.saturating_sub(1) as isize) as usize;
    let c = (at.1 as isize + dc).clamp(0, nc.saturating_sub(1) as isize) as usize;
    (r, c)
}

/// Write the cell being typed into, if any.
fn commit_edit_text(chart: &Chart, s: &mut ChartData, linked: bool, act: &mut Vec<Act>) {
    let Some(text) = s.edit.take() else { return };
    let (r, c) = s.at;
    if linked || chart.table.rows.get(r).and_then(|row| row.get(c)) == Some(&text) {
        return;
    }
    let mut next = chart.table.clone();
    next.rows[r][c] = text.chars().take(chart::MAX_CELL_CHARS).collect();
    act.push(Act::Table(next));
}

/// The header's and the gutter's menus.
fn menus(
    ui: &mut egui::Ui,
    chart: &Chart,
    s: &mut ChartData,
    menu: Menu,
    linked: bool,
    act: &mut Vec<Act>,
) {
    let t = &chart.table;
    let at = match menu {
        Menu::Header(_, p) | Menu::Row(_, p) => p,
    };
    let area = egui::Area::new(egui::Id::new("chart-data-menu"))
        .order(egui::Order::Tooltip)
        .fixed_pos(at)
        .show(ui.ctx(), |ui| {
            ui::menu_frame(4.0).show(ui, |ui| {
                ui.set_width(200.0);
                let row = |ui: &mut egui::Ui, glyph, label| {
                    ui::menu_row(
                        ui,
                        ui::MenuRow::new(glyph, label).enabled(!linked),
                        ui::MENU_ROW_H,
                    )
                    .clicked()
                };
                match menu {
                    Menu::Header(c, _) => {
                        let id = t.columns[c].id;
                        let category = if chart.kind == ChartKind::Scatter {
                            "Use as X"
                        } else {
                            "Use as category"
                        };
                        if ui::menu_row(
                            ui,
                            ui::MenuRow::new(icon::PENCIL_SIMPLE, "Rename"),
                            ui::MENU_ROW_H,
                        )
                        .clicked()
                        {
                            s.rename = Some((c, t.columns[c].name.clone()));
                            s.menu = None;
                        }
                        if chart.kind != ChartKind::Sankey {
                            if ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::TEXT_AA, category),
                                ui::MENU_ROW_H,
                            )
                            .clicked()
                            {
                                let mut next = chart.clone();
                                next.mapping.category = Some(id);
                                next.mapping.series.retain(|x| *x != id);
                                act.push(Act::Chart(Box::new(next)));
                                s.menu = None;
                            }
                            let shown = chart.mapping.series.contains(&id);
                            if ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::CHART, "Show as series").checked(shown),
                                ui::MENU_ROW_H,
                            )
                            .clicked()
                            {
                                let mut next = chart.clone();
                                if shown {
                                    next.mapping.series.retain(|x| *x != id);
                                } else {
                                    if next.mapping.category == Some(id) {
                                        next.mapping.category = None;
                                    }
                                    next.mapping.series.push(id);
                                }
                                act.push(Act::Chart(Box::new(next)));
                                s.menu = None;
                            }
                        }
                        ui::menu_sep(ui);
                        for (label, at) in
                            [("Insert column left", c), ("Insert column right", c + 1)]
                        {
                            if row(ui, icon::PLUS, label) && t.columns.len() < chart::MAX_COLUMNS {
                                let mut next = t.clone();
                                add_column(&mut next, at);
                                act.push(Act::Table(next));
                                s.menu = None;
                            }
                        }
                        for (label, up) in [("Sort ascending", true), ("Sort descending", false)] {
                            if row(
                                ui,
                                if up { icon::ARROW_UP } else { icon::ARROW_DOWN },
                                label,
                            ) {
                                act.push(Act::Table(sorted(t, c, up)));
                                s.menu = None;
                            }
                        }
                        ui::menu_sep(ui);
                        if ui::menu_row(
                            ui,
                            ui::MenuRow::new(icon::TRASH, "Delete column")
                                .danger(true)
                                .enabled(!linked),
                            ui::MENU_ROW_H,
                        )
                        .clicked()
                        {
                            act.push(Act::Chart(Box::new(without_column(chart, c))));
                            s.menu = None;
                        }
                    }
                    Menu::Row(r, _) => {
                        for (label, at) in [("Insert row above", r), ("Insert row below", r + 1)] {
                            if row(ui, icon::PLUS, label) && t.rows.len() < chart::MAX_ROWS {
                                let mut next = t.clone();
                                next.rows.insert(at, vec![String::new(); t.columns.len()]);
                                act.push(Act::Table(next));
                                s.menu = None;
                            }
                        }
                        ui::menu_sep(ui);
                        if ui::menu_row(
                            ui,
                            ui::MenuRow::new(icon::TRASH, "Delete row")
                                .danger(true)
                                .enabled(!linked),
                            ui::MENU_ROW_H,
                        )
                        .clicked()
                        {
                            let mut next = t.clone();
                            next.rows.remove(r);
                            act.push(Act::Table(next));
                            s.menu = None;
                        }
                    }
                }
            });
        });
    // A press outside the menu closes it.
    let outside = ui.input(|i| i.pointer.any_pressed())
        && ui
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|p| !area.response.rect.contains(p));
    if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        s.menu = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[&[&str]]) -> chart::Table {
        chart::Table {
            columns: (0..rows[0].len() as u32)
                .map(|i| chart::Column {
                    id: ColumnId(i),
                    name: format!("c{i}"),
                })
                .collect(),
            rows: rows
                .iter()
                .map(|r| r.iter().map(|c| (*c).to_string()).collect())
                .collect(),
        }
    }

    /// A pasted range lands from the cursor, growing the table to fit.
    #[test]
    fn a_paste_grows_the_table_to_fit() {
        let t = table(&[&["a", "1"], &["b", "2"]]);
        let cells = tsv_cells("x\t9\t8\r\ny\t7\t6\r\n");
        let next = pasted(&t, (1, 0), &cells);
        assert_eq!(next.columns.len(), 3);
        assert_eq!(next.rows.len(), 3);
        assert_eq!(next.rows[1], ["x", "9", "8"]);
        assert_eq!(next.rows[2], ["y", "7", "6"]);
        assert_eq!(next.rows[0], ["a", "1", ""]);
    }

    /// A number column sorts by value, text by letters, empty cells last.
    #[test]
    fn a_column_sorts_by_its_type() {
        let t = table(&[&["b", "10"], &["", "9"], &["a", ""], &["C", "100"]]);
        let by_num = sorted(&t, 1, true);
        let nums: Vec<&str> = by_num.rows.iter().map(|r| r[1].as_str()).collect();
        assert_eq!(nums, ["9", "10", "100", ""]);
        let by_text = sorted(&t, 0, false);
        let words: Vec<&str> = by_text.rows.iter().map(|r| r[0].as_str()).collect();
        assert_eq!(words, ["C", "b", "a", ""]);
    }

    /// A deleted column leaves the mapping and the series styles with it.
    #[test]
    fn a_deleted_column_leaves_the_mapping() {
        let chart = Chart::sample(ChartKind::Column);
        let second = chart.mapping.series[0];
        let at = chart.table.index_of(second).unwrap();
        let next = without_column(&chart, at);
        assert!(!next.mapping.series.contains(&second));
        assert_eq!(next.table.columns.len(), chart.table.columns.len() - 1);
        assert!(
            next.table
                .rows
                .iter()
                .all(|r| r.len() == next.table.columns.len())
        );
        assert_eq!(next.validate(), Ok(()));
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

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The window opens on a new chart; typing replaces the cell under the cursor,
    /// `Enter` commits it as one undo step and moves down, an arrow moves, and
    /// `Escape` twice closes the window.
    #[test]
    fn typing_in_the_grid_commits_a_cell_and_moves_down() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().unwrap();
        assert_eq!(
            app.chart_data.as_ref().map(|d| d.chart),
            Some(id),
            "opened on it"
        );
        for _ in 0..2 {
            pass(&ctx, &mut app, Vec::new());
        }
        pass(&ctx, &mut app, vec![key(egui::Key::ArrowRight)]);
        assert_eq!(
            app.chart_data.as_ref().map(|d| d.at),
            Some((0, 1)),
            "the grid has the keys"
        );
        pass(&ctx, &mut app, vec![egui::Event::Text("9".into())]);
        pass(&ctx, &mut app, Vec::new());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        let cell = |app: &OndinApp| {
            app.session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .unwrap()
                .table
                .rows[0][1]
                .clone()
        };
        assert_eq!(cell(&app), "9");
        assert_eq!(
            app.chart_data.as_ref().map(|d| d.at),
            Some((1, 1)),
            "moved down"
        );
        app.session.undo();
        assert_eq!(cell(&app), "42", "one undo step");
        for _ in 0..2 {
            pass(&ctx, &mut app, Vec::new());
        }
        pass(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(app.chart_data.is_none(), "Escape with no edit closes it");
    }
}
