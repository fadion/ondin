//! The chart data window (`charts.md`, *The data window*): a card over the
//! canvas, where Clean up's hangs, holding a chart's table as a grid. Open, as
//! Clean up is: the canvas stays live under it, and each committed cell, paste,
//! clear, insert or delete is one `SetChart` (*Edit chart data*), so the chart
//! redraws as the table changes.
//!
//! The grid is one focusable region that locks the arrows, `Tab` and `Escape`
//! while it has the keyboard, so the canvas keymap stays quiet; a cell being
//! typed into turns editable in place (`ui::label_edit_set`).

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;
use eframe::egui;
use ondin_core::chart::{self, Chart, ChartKind, ColumnId};
use ondin_core::{NodeId, Operation, Transaction};

/// The card's width; narrower when the canvas is ([`OndinApp::chart_data_ui`]).
pub(crate) const W: f32 = 640.0;
/// The grid's height, whatever the table holds, so the card keeps its size.
const GRID_H: f32 = 240.0;
const PAD: f32 = crate::settings::CANVAS_CARD_PAD;
/// Between the toolbar, the linked line and the grid: Clean up's.
const GAP: f32 = 10.0;
/// A cell's width and height, the row-number gutter's width.
const COL_W: f32 = 112.0;
const ROW_H: f32 = ui::MENU_ROW_H;
const GUTTER: f32 = 40.0;
/// The space a cell's text keeps from its edges: a [`ui::label_edit_set`]'s,
/// so a cell typed into keeps its text where it was.
const CELL_PAD: f32 = ui::INLINE_EDIT_PAD_X;
/// The category's key mark in a header.
const KEY_PT: f32 = 11.0;
/// The grid's focus id.
const GRID_ID: &str = "chart-data-grid";
/// The undo step every edit makes.
const STEP: &str = "Edit chart data";

/// A cell by row and column index in the table.
pub(crate) type Cell = (usize, usize);

/// The window while it is open.
#[derive(Clone, Debug)]
pub(crate) struct ChartData {
    /// The chart it shows, until another chart is selected alone or it is gone.
    pub(crate) chart: NodeId,
    /// The cursor, and the other corner of the selected range.
    pub(crate) at: Cell,
    pub(crate) anchor: Cell,
    /// A cell being typed into, kept until committed or abandoned.
    edit: Option<Typing>,
    /// A header being renamed: its column, by id so a column added or removed
    /// from outside never moves it, and its text.
    rename: Option<(ColumnId, String)>,
    /// The open header or row menu.
    menu: Option<Menu>,
    /// Ask for the keyboard on the next pass: the cell or header being typed
    /// into takes it, else the grid. Asked once, so a press elsewhere keeps it.
    focus: bool,
    /// Scroll the cursor into view on the next pass.
    reveal: bool,
}

/// A cell being typed into: its text, its cell by row and column id, and the
/// table it was started on. Any other change to the table (an undo, an import,
/// an agent's edit) drops it, so `Enter` never writes into a cell that moved.
#[derive(Clone, Debug)]
struct Typing {
    text: String,
    row: usize,
    col: ColumnId,
    base: chart::Table,
}

impl Typing {
    /// Typing `text` into `t`'s cell `at`; `None` when there is no such cell.
    fn new(t: &chart::Table, at: Cell, text: String) -> Option<Self> {
        let col = t.columns.get(at.1)?.id;
        t.rows.get(at.0)?;
        Some(Self {
            text,
            row: at.0,
            col,
            base: t.clone(),
        })
    }

    /// Its cell in `t`, while `t` is still the table it was started on.
    fn cell(&self, t: &chart::Table) -> Option<Cell> {
        if *t != self.base {
            return None;
        }
        self.at(t)
    }

    /// Its cell in `t`, unchecked against the table it was started on: for a
    /// pass that has already dropped a stale edit ([`grid`]).
    fn at(&self, t: &chart::Table) -> Option<Cell> {
        let c = t.index_of(self.col)?;
        t.rows.get(self.row)?.get(c)?;
        Some((self.row, c))
    }
}

/// An open menu, held on what it was opened on so a table changed from outside
/// never moves it onto another column or row: a header's column by id, a row
/// by its place and its cells, closing once that place holds other cells.
#[derive(Clone, Debug, PartialEq)]
enum Menu {
    Header(ColumnId, egui::Pos2),
    Row(usize, Vec<String>, egui::Pos2),
}

impl Menu {
    /// The column or row it acts on in `t`; `None` once that is gone or moved.
    fn place(&self, t: &chart::Table) -> Option<usize> {
        match self {
            Menu::Header(id, _) => t.index_of(*id),
            Menu::Row(r, cells, _) => (t.rows.get(*r) == Some(cells)).then_some(*r),
        }
    }
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

fn role_of(chart: &Chart, col: ColumnId, swatch: Option<egui::Color32>) -> Role {
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
        return Role::Series(swatch);
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
    // A category mark's index counts the categories, which leave out the rows
    // with an empty category; a scatter's point and a flow count table rows.
    let by_row =
        matches!(key.role, R::Flow) || (key.role == R::Point && chart.kind == ChartKind::Scatter);
    let row = match at(chart.mapping.category) {
        Some(c) if !by_row => t
            .rows
            .iter()
            .enumerate()
            .filter(|(_, cells)| cells.get(c).is_some_and(|v| !v.trim().is_empty()))
            .nth(key.index as usize)
            .map_or(last, |(r, _)| r),
        _ => (key.index as usize).min(last),
    };
    let first_series = at(chart.mapping.series.first().copied());
    let cell = match key.role {
        // A pie's value label is its slice's.
        R::ValueLabel if chart.kind == ChartKind::Pie => first_series.map(|c| (row, c)),
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

/// What a paste left out to keep the table inside its limits.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Cut {
    rows: usize,
    columns: usize,
    cells: usize,
}

impl Cut {
    /// The status line's note, *Pasted what fits · 2,000 rows left out*;
    /// `None` when nothing was cut.
    fn note(self) -> Option<String> {
        let count = |n: usize, one: &str, many: &str| {
            format!("{} {}", chart::grouped(n), if n == 1 { one } else { many })
        };
        let mut parts = Vec::new();
        if self.rows > 0 {
            parts.push(format!("{} left out", count(self.rows, "row", "rows")));
        }
        if self.columns > 0 {
            parts.push(format!(
                "{} left out",
                count(self.columns, "column", "columns")
            ));
        }
        if self.cells > 0 {
            parts.push(format!(
                "{} cut to {} characters",
                count(self.cells, "cell", "cells"),
                chart::MAX_CELL_CHARS
            ));
        }
        (!parts.is_empty()).then(|| format!("Pasted what fits \u{b7} {}", parts.join(", ")))
    }
}

/// `table` with `cells` written from `at` down and right, growing the table to
/// fit (a paste from a spreadsheet); held to the table's limits, with what
/// they left out.
pub(crate) fn pasted(table: &chart::Table, at: Cell, cells: &[Vec<String>]) -> (chart::Table, Cut) {
    let mut t = table.clone();
    let mut cut = Cut::default();
    let width = cells.iter().map(Vec::len).max().unwrap_or(0);
    let fit_c = (at.1 + width).min(chart::MAX_COLUMNS.max(t.columns.len()));
    cut.columns = (at.1 + width).saturating_sub(fit_c);
    while t.columns.len() < fit_c {
        let end = t.columns.len();
        add_column(&mut t, end);
    }
    let rows = (at.0 + cells.len()).min(chart::MAX_ROWS.max(t.rows.len()));
    cut.rows = (at.0 + cells.len()).saturating_sub(rows);
    while t.rows.len() < rows {
        t.rows.push(vec![String::new(); t.columns.len()]);
    }
    for (dr, line) in cells.iter().enumerate() {
        for (dc, cell) in line.iter().enumerate() {
            let (r, c) = (at.0 + dr, at.1 + dc);
            if r < t.rows.len() && c < t.columns.len() {
                if cell.chars().nth(chart::MAX_CELL_CHARS).is_some() {
                    cut.cells += 1;
                }
                t.rows[r][c] = cell.chars().take(chart::MAX_CELL_CHARS).collect();
            }
        }
    }
    (t, cut)
}

/// Tab-separated text read as rows of cells, as a spreadsheet copies them: a
/// cell holding a tab, a line break or a quote comes quoted, its quotes
/// doubled. `table_io`'s TSV reader quotes alike but drops blank lines, which
/// in a pasted range are empty rows that keep the rows below in place.
pub(crate) fn tsv_cells(text: &str) -> Vec<Vec<String>> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    let text = text.strip_suffix('\r').unwrap_or(text);
    let mut out = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            match ch {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => quoted = false,
                c => field.push(c),
            }
            continue;
        }
        match ch {
            '"' if field.is_empty() => quoted = true,
            '\t' => row.push(std::mem::take(&mut field)),
            '\r' | '\n' => {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                out.push(std::mem::take(&mut row));
            }
            c => field.push(c),
        }
    }
    row.push(field);
    out.push(row);
    out
}

/// The range's cells as tab-separated text, quoted as [`tsv_cells`] reads them.
fn tsv_of(
    table: &chart::Table,
    rows: std::ops::RangeInclusive<usize>,
    cols: std::ops::RangeInclusive<usize>,
) -> String {
    let quoted = |cell: &str| {
        if cell.contains(['\t', '\n', '\r', '"']) {
            format!("\"{}\"", cell.replace('"', "\"\""))
        } else {
            cell.to_string()
        }
    };
    rows.filter_map(|r| table.rows.get(r))
        .map(|row| {
            cols.clone()
                .filter_map(|c| row.get(c).map(|c| quoted(c)))
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
        // One card over the canvas's top at a time.
        self.close_find();
        self.colour_review.open = false;
        self.batch_rename = None;
        self.clean_up = None;
        self.fill_content = None;
        self.chart_data = Some(ChartData::new(id, at));
    }

    /// Close the window, dropping a cell being typed into.
    pub(crate) fn close_chart_data(&mut self) {
        self.chart_data = None;
    }

    /// `Escape` with the window open and nothing in it holding the keyboard:
    /// close it. The grid and a cell being typed into answer their own.
    pub(crate) fn escape_chart_data(&mut self, ctx: &egui::Context) -> bool {
        if self.chart_data.is_none() || ctx.memory(|m| m.focused().is_some()) {
            return false;
        }
        self.chart_data = None;
        true
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

    /// The window, each frame: moved to another chart when that one alone is
    /// selected, and closed when its chart is gone. Selecting anything else, or
    /// nothing, leaves it on its chart.
    pub(crate) fn chart_data_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
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
        // Selecting another chart moves the window there.
        if let [other] = self.session.selection.ids()
            && *other != id
            && self
                .session
                .doc
                .get(*other)
                .is_some_and(|n| n.chart().is_some())
        {
            self.chart_data = Some(ChartData::new(*other, (0, 0)));
            return;
        }
        let Some(chart) = self.session.doc.get(id).and_then(|n| n.chart()).cloned() else {
            self.chart_data = None;
            return;
        };
        let name = self
            .session
            .doc
            .get(id)
            .map(|n| n.name().to_string())
            .unwrap_or_default();
        let w = W.min(canvas.width() - 32.0).max(320.0);
        let linked = chart.source.is_some();
        // Each column's series swatch, a binding read where the chart is.
        let swatches: Vec<Option<egui::Color32>> = chart
            .table
            .columns
            .iter()
            .map(|c| {
                crate::panels::chart_colour(&self.session.doc, id, &chart, c.id)
                    .map(ui::color_to_egui)
            })
            .collect();
        let mut act: Vec<Act> = Vec::new();
        let Some(mut state) = self.chart_data.take() else {
            return;
        };
        let hold = crate::settings::CardHold::Open;
        crate::settings::canvas_card(ctx, "chart-data", canvas, w, hold, |ui| {
            if crate::settings::card_head_noted(ui, w, "Chart data", Some(&name)) {
                act.push(Act::Close);
            }
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: PAD as i8,
                    right: PAD as i8,
                    top: 0,
                    bottom: PAD as i8,
                })
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(ui::CARD_COL_GAP, GAP);
                    grid(ui, &chart, &swatches, &mut state, linked, &mut act);
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
                Act::Note(text) => self.session.info(text),
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
    /// A word on the status line.
    Note(String),
}

/// A toolbar button: its glyph and word, as wide as they need.
fn tool_button(ui: &mut egui::Ui, glyph: &str, label: &str) -> egui::Response {
    let w = ui::action_button_w(ui.ctx(), label);
    let size = egui::vec2(w, ui::CONTROL_H);
    ui::action_button(ui, glyph, label, ui::FieldButton::Off, size)
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

/// The grid under its toolbar (*Add row*, *Add column*, and *Import…* at the
/// right): the header row and the row gutter held at the viewport's edges over
/// the scrolled cells, and past the table's last row and column a dim grid
/// filling the view, which neither scrolls nor takes a click.
fn grid(
    ui: &mut egui::Ui,
    chart: &Chart,
    swatches: &[Option<egui::Color32>],
    s: &mut ChartData,
    linked: bool,
    act: &mut Vec<Act>,
) {
    let t = &chart.table;
    let (nr, nc) = (t.rows.len(), t.columns.len());
    // The table's size once this pass's edit lands, so the cursor may stand on
    // a row or column added just now.
    let (mut fit_r, mut fit_c) = (nr, nc);
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if tool_button(ui, icon::IMPORT, "Import…")
                .on_hover_text("Replace the table with a CSV, TSV or JSON file")
                .clicked()
            {
                act.push(Act::Import);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if linked {
                    return;
                }
                let more_rows = nr < chart::MAX_ROWS;
                let resp = ui::disable_unless(ui, more_rows, |ui| {
                    tool_button(ui, icon::ROWS_PLUS_BOTTOM, "Add row")
                })
                .inner
                .on_disabled_hover_text(format!("At most {} rows", chart::MAX_ROWS));
                if resp.clicked() {
                    let mut next = with_typed(t, s);
                    next.rows.push(vec![String::new(); nc]);
                    s.at = (nr, s.at.1.min(nc.saturating_sub(1)));
                    s.anchor = s.at;
                    s.reveal = true;
                    fit_r = nr + 1;
                    act.push(Act::Table(next));
                }
                let more_cols = nc < chart::MAX_COLUMNS;
                let resp = ui::disable_unless(ui, more_cols, |ui| {
                    tool_button(ui, icon::COLUMNS_PLUS_RIGHT, "Add column")
                })
                .inner
                .on_disabled_hover_text(format!("At most {} columns", chart::MAX_COLUMNS));
                if resp.clicked() {
                    let mut next = with_typed(t, s);
                    add_column(&mut next, nc);
                    s.at = (s.at.0, nc);
                    s.anchor = s.at;
                    s.reveal = true;
                    fit_c = nc + 1;
                    act.push(Act::Table(next));
                }
            });
        });
    });
    if linked {
        ui.label(
            egui::RichText::new(linked_line(chart))
                .size(ui::SMALL_PT)
                .color(theme::text::dim()),
        );
    }
    // Keep the cursor inside the table as it shrinks under an undo, and drop a
    // menu or rename whose column or row is gone or moved.
    s.at = (
        s.at.0.min(fit_r.saturating_sub(1)),
        s.at.1.min(fit_c.saturating_sub(1)),
    );
    s.anchor = (
        s.anchor.0.min(fit_r.saturating_sub(1)),
        s.anchor.1.min(fit_c.saturating_sub(1)),
    );
    if s.rename
        .as_ref()
        .is_some_and(|(id, _)| t.index_of(*id).is_none())
    {
        s.rename = None;
    }
    if s.menu.as_ref().is_some_and(|m| m.place(t).is_none()) {
        s.menu = None;
    }
    // A table changed from outside (an undo, an import, an agent's edit) drops
    // the cell being typed into, whose cell may have moved or gone.
    if s.edit.as_ref().is_some_and(|e| e.cell(t).is_none()) {
        s.edit = None;
    }

    let grid_id = egui::Id::new(GRID_ID);
    let content = egui::vec2(GUTTER + COL_W * nc as f32, ROW_H * (nr as f32 + 1.0));
    let editing = s.edit.is_some() || s.rename.is_some();
    keys(ui, chart, s, linked, act, grid_id, editing);

    // Scrolled by the wheel and the cursor, its bars hidden as Settings' are.
    let scroll = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .max_height(GRID_H)
        .min_scrolled_height(GRID_H)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .id_salt("chart-data-scroll");
    scroll.show_viewport(ui, |ui, view| {
        let (rect, _) = ui.allocate_exact_size(content, egui::Sense::hover());
        let resp = ui.interact(rect, grid_id, egui::Sense::click_and_drag());
        // A cell or header being typed into takes the keyboard below.
        if s.focus && s.edit.is_none() && s.rename.is_none() {
            resp.request_focus();
            s.focus = false;
        }
        if resp.has_focus() {
            // The card holds `Tab` among its controls; the grid's moves its cursor.
            ui::keep_tab(ui.ctx(), grid_id);
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
                (None, Some(c)) if resp.secondary_clicked() => {
                    s.menu = Some(Menu::Header(t.columns[c].id, p));
                }
                (Some(r), None) if resp.secondary_clicked() => {
                    s.menu = Some(Menu::Row(r, t.rows[r].clone(), p));
                }
                (None, Some(c)) => {
                    // A header click selects its column.
                    commit_edit_text(chart, s, linked, act);
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
                (Some(_), Some(_)) if !linked => {
                    let text = t.rows[s.at.0][s.at.1].clone();
                    s.edit = Typing::new(t, s.at, text);
                    s.focus = true;
                }
                (None, Some(c)) if !linked => {
                    s.rename = Some((t.columns[c].id, t.columns[c].name.clone()));
                    s.focus = true;
                }
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
        let typing_at = s.edit.as_ref().and_then(|e| e.at(t));
        // The dim grid past the table: the cells' own lines on the card's ground,
        // only those in view.
        let v_last = (view.max.y / ROW_H).ceil() as usize + 1;
        let vc_first = ((view.min.x - GUTTER).max(0.0) / COL_W).floor() as usize;
        let vc_last = ((view.max.x - GUTTER).max(0.0) / COL_W).ceil() as usize + 1;
        let faint = egui::Stroke::new(1.0, theme::color::card_border());
        for r in first..v_last {
            for c in vc_first..vc_last {
                if r >= nr || c >= nc {
                    p.rect_stroke(cell_rect(r, c), 0.0, faint, egui::StrokeKind::Inside);
                }
            }
        }
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
                if !text.is_empty() && typing_at != Some((r, c)) {
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
        // A header's tooltips by where they answer: a role mark's, then a cut
        // name's, the first that holds the pointer.
        let mut tips: Vec<(egui::Rect, String)> = Vec::new();
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
            // A header being renamed is its field, drawn below.
            if s.rename.as_ref().is_some_and(|(id, _)| *id == column.id) {
                continue;
            }
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
            let role = role_of(chart, column.id, swatches.get(c).copied().flatten());
            if let Role::Series(Some(colour)) = role {
                let sw = egui::Rect::from_center_size(
                    egui::pos2(x + 4.0, hr.center().y),
                    egui::vec2(8.0, 8.0),
                );
                clip.rect_filled(sw, 2.0, colour);
                x += 13.0;
            }
            let ink = match role {
                Role::Unmapped => theme::text::dim(),
                _ => color::text(),
            };
            // The role mark against the right edge: the category's key, or a
            // Sankey column's word.
            let mark = match role {
                Role::Category => Some((
                    clip.layout_no_wrap(
                        icon::KEY.into(),
                        theme::icon_font(KEY_PT),
                        theme::text::faint(),
                    ),
                    Some(if chart.kind == ChartKind::Scatter {
                        "The chart's X values"
                    } else {
                        "The chart's categories"
                    }),
                )),
                Role::Named(n) => Some((
                    clip.layout_no_wrap(
                        n.to_string(),
                        egui::FontId::proportional(ui::SMALL_PT - 1.5),
                        theme::text::faint(),
                    ),
                    None,
                )),
                _ => None,
            };
            let mut name_right = hr.right() - CELL_PAD;
            if let Some((g, tip)) = mark {
                let at = egui::pos2(name_right - g.size().x, hr.center().y - g.size().y / 2.0);
                let mark_rect = egui::Rect::from_min_size(at, g.size());
                clip.galley(at, g, theme::text::faint());
                if let Some(tip) = tip {
                    tips.push((mark_rect.expand(2.0), tip.to_string()));
                }
                name_right = mark_rect.left() - 4.0;
            }
            // The name stops short of the mark, an ellipsis where it is cut and
            // the whole name its tooltip.
            let mut job = egui::text::LayoutJob::simple_singleline(
                column.name.clone(),
                egui::FontId::proportional(ui::SMALL_PT),
                ink,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width((name_right - x).max(0.0));
            let name_g = clip.layout_job(job);
            if name_g.elided {
                tips.push((hr, column.name.clone()));
            }
            clip.galley(
                egui::pos2(x, hr.center().y - name_g.size().y / 2.0),
                name_g,
                ink,
            );
        }
        if let Some(at) = resp.hover_pos()
            && let Some((_, tip)) = tips.iter().find(|(r, _)| r.contains(at))
        {
            resp.clone().on_hover_text_at_pointer(tip);
        }
        p.rect_filled(
            egui::Rect::from_min_size(egui::pos2(gx, hy), egui::vec2(GUTTER, ROW_H)),
            0.0,
            theme::color::card(),
        );

        // The cell being typed into turns editable in place: its ground and the
        // cursor's square edge stay the grid's, and its text where it was drawn.
        // It takes the keyboard once; losing it to a press elsewhere commits it,
        // as a layer's rename does, so the canvas keymap is never muted by it.
        let mut lost = false;
        if let (Some(e), Some((r, c))) = (&mut s.edit, typing_at) {
            let rc = cell_rect(r, c);
            let align = match numeric.get(c) {
                Some(true) => egui::Align::Max,
                _ => egui::Align::Min,
            };
            let id = egui::Id::new((GRID_ID, "cell"));
            let resp = ui::label_edit_set(ui, rc, &mut e.text, id, ui::CONTROL_PT, align);
            if s.focus {
                resp.request_focus();
                s.focus = false;
            }
            lost = resp.lost_focus();
        }
        if lost {
            commit_edit_text(chart, s, linked, act);
        }
        // A header being renamed: a cell's ground under the cursor's edge.
        let mut lost = false;
        if let Some((id, text)) = &mut s.rename
            && let Some(c) = t.index_of(*id)
        {
            let hr = egui::Rect::from_min_size(
                egui::pos2(cell_rect(0, c).left(), hy),
                egui::vec2(COL_W, ROW_H),
            );
            let edge = egui::Stroke::new(1.5, color::accent());
            p.rect_filled(hr, 0.0, theme::color::field());
            p.rect_stroke(hr, 0.0, edge, egui::StrokeKind::Inside);
            let id = egui::Id::new((GRID_ID, "rename"));
            let resp = ui::label_edit_set(ui, hr, text, id, ui::SMALL_PT, egui::Align::Min);
            if s.focus {
                resp.request_focus();
                s.focus = false;
            }
            lost = resp.lost_focus();
        }
        if lost {
            commit_rename(t, s, act);
        }
    });

    if let Some(menu) = s.menu.clone() {
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
    // An open menu holds the keyboard: `Escape` closes only it, and the
    // arrows leave the grid under it alone.
    if s.menu.is_some() {
        if pressed(egui::Key::Escape) {
            s.menu = None;
        }
        return;
    }
    if editing {
        if s.rename.is_some() {
            if pressed(egui::Key::Escape) {
                s.rename = None;
                s.focus = true;
            } else if pressed(egui::Key::Enter) || pressed(egui::Key::Tab) {
                commit_rename(t, s, act);
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
        s.reveal = true;
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
        s.edit = Typing::new(t, s.at, t.rows[s.at.0][s.at.1].clone());
        s.focus = true;
        return;
    }
    // Typing replaces the cell: every character this frame brought, the editor
    // being drawn only after this pass's events are read.
    if !command {
        let typed: String = events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if !typed.is_empty() {
            s.edit = Typing::new(t, s.at, typed);
            s.focus = true;
            return;
        }
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
                let (next, cut) = pasted(t, at, &cells);
                let (fit_r, fit_c) = (next.rows.len(), next.columns.len());
                act.push(Act::Table(next));
                if let Some(note) = cut.note() {
                    act.push(Act::Note(note));
                }
                let h = cells.len().max(1);
                let w = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
                s.anchor = at;
                s.at = (
                    (at.0 + h - 1).min(fit_r.saturating_sub(1)),
                    (at.1 + w - 1).min(fit_c.saturating_sub(1)),
                );
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

/// `t` with the cell being typed into written, taking the typing: an edit that
/// grows the table carries it, rather than a second write from the old table
/// dropping it.
fn with_typed(t: &chart::Table, s: &mut ChartData) -> chart::Table {
    let mut next = t.clone();
    if let Some(e) = s.edit.take()
        && let Some((r, c)) = e.cell(t)
    {
        next.rows[r][c] = e.text.chars().take(chart::MAX_CELL_CHARS).collect();
    }
    next
}

/// Write the cell being typed into, if any, into the cell it was started on;
/// dropped when that cell is gone or the table changed under it.
fn commit_edit_text(chart: &Chart, s: &mut ChartData, linked: bool, act: &mut Vec<Act>) {
    let Some(e) = s.edit.take() else { return };
    if linked {
        return;
    }
    let Some((r, c)) = e.cell(&chart.table) else {
        return;
    };
    let text: String = e.text.chars().take(chart::MAX_CELL_CHARS).collect();
    let mut next = chart.table.clone();
    let Some(cell) = next.rows.get_mut(r).and_then(|row| row.get_mut(c)) else {
        return;
    };
    if *cell == text {
        return;
    }
    *cell = text;
    // A row menu open on this row holds the cells this write leaves there,
    // or it would read its own row as moved and close.
    if let Some(Menu::Row(at, cells, _)) = &mut s.menu
        && *at == r
    {
        cells.clone_from(&next.rows[r]);
    }
    act.push(Act::Table(next));
}

/// Write the header being renamed, if any, onto its column wherever it now
/// sits: a name left empty or unchanged, or a column gone, writes nothing.
fn commit_rename(t: &chart::Table, s: &mut ChartData, act: &mut Vec<Act>) {
    let Some((id, name)) = s.rename.take() else {
        return;
    };
    let name: String = name.trim().chars().take(chart::MAX_CELL_CHARS).collect();
    let Some(c) = t.index_of(id) else {
        return;
    };
    if name.is_empty() || t.columns[c].name == name {
        return;
    }
    let mut next = t.clone();
    next.columns[c].name = name;
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
    // `grid` has closed a menu whose column or row moved; a row's cells may
    // already be ahead of `t` by this frame's own typing, so only its bounds
    // are checked here.
    let place = match &menu {
        Menu::Header(id, _) => t.index_of(*id),
        Menu::Row(r, ..) => (*r < t.rows.len()).then_some(*r),
    };
    let Some(place) = place else {
        s.menu = None;
        return;
    };
    let at = match menu {
        Menu::Header(_, p) | Menu::Row(_, _, p) => p,
    };
    // Kept on screen by the size it drew at last frame, its rows varying by menu.
    let area_id = egui::Id::new("chart-data-menu");
    let size = ui
        .ctx()
        .memory(|m| m.area_rect(area_id))
        .map_or(egui::vec2(208.0, 0.0), |r| r.size());
    let area = egui::Area::new(area_id)
        .order(egui::Order::Tooltip)
        .fixed_pos(ui::menu_place(ui.ctx(), at, size))
        .show(ui.ctx(), |ui| {
            ui::menu_frame(4.0).show(ui, |ui| {
                ui.set_width(200.0);
                // A row that edits the table, dimmed with its reason on a linked
                // chart or where `room` says the table is full.
                let lock = linked.then(|| linked_line(chart));
                let row = |ui: &mut egui::Ui, row: ui::MenuRow, room: Option<String>| {
                    let why = lock.clone().or(room);
                    let resp = ui::menu_row(ui, row.enabled(why.is_none()), ui::MENU_ROW_H);
                    match why {
                        Some(why) => {
                            resp.on_hover_text(why);
                            false
                        }
                        None => resp.clicked(),
                    }
                };
                let full_cols = (t.columns.len() >= chart::MAX_COLUMNS)
                    .then(|| format!("At most {} columns", chart::MAX_COLUMNS));
                let full_rows = (t.rows.len() >= chart::MAX_ROWS)
                    .then(|| format!("At most {} rows", chart::MAX_ROWS));
                match menu {
                    Menu::Header(id, _) => {
                        let c = place;
                        let category = if chart.kind == ChartKind::Scatter {
                            "Use as X"
                        } else {
                            "Use as category"
                        };
                        if row(ui, ui::MenuRow::new(icon::PENCIL_SIMPLE, "Rename"), None) {
                            commit_edit_text(chart, s, linked, act);
                            s.rename = Some((id, t.columns[c].name.clone()));
                            s.focus = true;
                            s.menu = None;
                        }
                        if chart.kind != ChartKind::Sankey {
                            let is_category = chart.mapping.category == Some(id);
                            let resp = ui::menu_row(
                                ui,
                                ui::MenuRow::new(icon::KEY, category).enabled(!is_category),
                                ui::MENU_ROW_H,
                            );
                            let resp = if is_category {
                                resp.on_hover_text(if chart.kind == ChartKind::Scatter {
                                    "Already the X column"
                                } else {
                                    "Already the category column"
                                })
                            } else {
                                resp
                            };
                            if resp.clicked() {
                                let mut next = chart.clone();
                                next.table = with_typed(t, s);
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
                                next.table = with_typed(t, s);
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
                        // Each table edit carries the cell being typed into, in
                        // its own cell, before the rows or columns move.
                        for (label, at) in
                            [("Insert column left", c), ("Insert column right", c + 1)]
                        {
                            if row(ui, ui::MenuRow::new(icon::PLUS, label), full_cols.clone()) {
                                let mut next = with_typed(t, s);
                                add_column(&mut next, at);
                                act.push(Act::Table(next));
                                s.menu = None;
                            }
                        }
                        for (label, up) in [("Sort ascending", true), ("Sort descending", false)] {
                            let glyph = if up { icon::ARROW_UP } else { icon::ARROW_DOWN };
                            if row(ui, ui::MenuRow::new(glyph, label), None) {
                                act.push(Act::Table(sorted(&with_typed(t, s), c, up)));
                                s.menu = None;
                            }
                        }
                        ui::menu_sep(ui);
                        let delete = ui::MenuRow::new(icon::TRASH, "Delete column").danger(true);
                        if row(ui, delete, None) {
                            let typed = Chart {
                                table: with_typed(t, s),
                                ..chart.clone()
                            };
                            act.push(Act::Chart(Box::new(without_column(&typed, c))));
                            s.menu = None;
                        }
                    }
                    Menu::Row(..) => {
                        let r = place;
                        for (label, at) in [("Insert row above", r), ("Insert row below", r + 1)] {
                            if row(ui, ui::MenuRow::new(icon::PLUS, label), full_rows.clone()) {
                                let mut next = with_typed(t, s);
                                next.rows.insert(at, vec![String::new(); t.columns.len()]);
                                act.push(Act::Table(next));
                                s.menu = None;
                            }
                        }
                        ui::menu_sep(ui);
                        let delete = ui::MenuRow::new(icon::TRASH, "Delete row").danger(true);
                        if row(ui, delete, None) {
                            let mut next = with_typed(t, s);
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
        let (next, cut) = pasted(&t, (1, 0), &cells);
        assert_eq!(cut, Cut::default(), "nothing cut");
        assert_eq!(next.columns.len(), 3);
        assert_eq!(next.rows.len(), 3);
        assert_eq!(next.rows[1], ["x", "9", "8"]);
        assert_eq!(next.rows[2], ["y", "7", "6"]);
        assert_eq!(next.rows[0], ["a", "1", ""]);
    }

    /// A paste past the row limit keeps what fits and says how much it left out.
    /// flip: `pasted` leaving `cut.rows` at zero → fails at "one row left out".
    #[test]
    fn a_paste_past_the_limits_says_what_it_cut() {
        let t = table(&[&["a", "1"]]);
        let cells: Vec<Vec<String>> = (0..=chart::MAX_ROWS).map(|i| vec![i.to_string()]).collect();
        let (next, cut) = pasted(&t, (0, 0), &cells);
        assert_eq!(next.rows.len(), chart::MAX_ROWS);
        assert_eq!(cut.rows, 1, "one row left out");
        assert_eq!(
            cut.note().as_deref(),
            Some("Pasted what fits \u{b7} 1 row left out")
        );
        let (_, cut) = pasted(&t, (0, 0), &[vec!["x".repeat(chart::MAX_CELL_CHARS + 1)]]);
        assert_eq!(cut.cells, 1, "a long cell is cut");
    }

    /// A spreadsheet's quoted cells paste whole, and the window's copy quotes
    /// them back.
    /// flip: `tsv_cells` splitting on every line break → fails at "one row".
    #[test]
    fn a_quoted_paste_keeps_its_line_breaks() {
        let cells = tsv_cells("\"North\nregion\"\t5\n\"12\"\" pipe\"\t\n\nlast\t1\r\n");
        assert_eq!(
            cells,
            [
                vec!["North\nregion".to_string(), "5".into()],
                vec!["12\" pipe".to_string(), String::new()],
                vec![String::new()],
                vec!["last".to_string(), "1".into()],
            ],
            "one row per record, the blank one kept"
        );
        let t = table(&[&["North\nregion", "5"], &["12\" pipe", "x\ty"]]);
        let back = tsv_cells(&tsv_of(&t, 0..=1, 0..=1));
        assert_eq!(back, t.rows, "the copy reads back as it was");
    }

    /// A category mark's index counts the categories, so a row with an empty
    /// category above it does not shift the cell; a pie's value label opens its
    /// slice's value.
    /// flip: `cell_of` reading the index as the table row → fails at "Mar's row".
    #[test]
    fn a_mark_opens_its_own_row_past_an_empty_category() {
        let mut c = Chart::sample(ChartKind::Column);
        c.table = table(&[&["Jan", "10"], &["", "20"], &["Mar", "30"]]);
        c.mapping.category = Some(ColumnId(0));
        c.mapping.series = vec![ColumnId(1)];
        let key = |role, series, index| {
            Some(chart::PartKey {
                role,
                series,
                index,
            })
        };
        assert_eq!(
            cell_of(&c, key(chart::PartRole::Bar, 1, 1)),
            (2, 1),
            "Mar's row"
        );
        c.kind = ChartKind::Pie;
        assert_eq!(
            cell_of(&c, key(chart::PartRole::ValueLabel, 0, 0)),
            (0, 1),
            "the slice's value, not the category"
        );
    }

    /// A cell being typed into whose row a *Delete row* emptied writes nothing
    /// rather than indexing a table with no rows. Three defences each hold it
    /// alone (`Typing::cell`'s base check, `Typing::at`'s bounds, the `get_mut`
    /// in `commit_edit_text`), so no single flip bites.
    #[test]
    fn typing_into_a_deleted_row_writes_nothing() {
        let one = table(&[&["a", "1"]]);
        let mut chart = Chart::sample(ChartKind::Column);
        chart.table = one.clone();
        let mut s = ChartData::new(NodeId { actor: 1, seq: 1 }, (0, 0));
        s.edit = Typing::new(&one, (0, 0), "x".into());
        chart.table.rows.clear();
        let mut act = Vec::new();
        commit_edit_text(&chart, &mut s, false, &mut act);
        assert!(act.is_empty(), "nothing written");
        assert!(s.edit.is_none(), "the typing is dropped");
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

    /// A header name too long for its cell stops short of the category's key.
    /// flip: the header's name galley laid out with `layout_no_wrap` → fails at "clear of the mark".
    #[test]
    fn a_long_header_name_stops_short_of_its_role_mark() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut sample = Chart::sample(ChartKind::Column);
        let long = "A category name far too long for one header cell";
        sample.table.columns[0].name = long.into();
        app.insert_chart_at(ondin_core::kurbo::Point::new(-4000.0, 0.0), sample);
        let mut frame = eframe::Frame::_new_kittest();
        let mut out = Vec::new();
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                ..Default::default()
            };
            out = ctx
                .run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame))
                .shapes;
        }
        fn find(shape: &egui::Shape, s: &str) -> Option<egui::Rect> {
            match shape {
                egui::Shape::Text(t) if t.galley.text().starts_with(s) => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                egui::Shape::Vec(v) => v.iter().find_map(|x| find(x, s)),
                _ => None,
            }
        }
        let at = |s: &str| out.iter().find_map(|c| find(&c.shape, s));
        let name = at("A category name").expect("the header's name is drawn");
        let mark = at(icon::KEY).expect("the key is drawn");
        assert!(
            (name.center().y - mark.center().y).abs() < ROW_H / 2.0,
            "the same header: {name:?} {mark:?}"
        );
        assert!(
            name.right() <= mark.left(),
            "clear of the mark: {name:?} {mark:?}"
        );
    }

    /// A right-click on a header opens its menu; a click selects the column.
    /// flip: dropping the header's secondary-click arm in the grid's click
    /// match → fails at "a right-click opens it".
    #[test]
    fn a_right_click_opens_the_header_menu() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new());
        }
        let grid = ctx
            .read_response(egui::Id::new(GRID_ID))
            .expect("the grid is drawn")
            .rect;
        let y = grid.top() + ROW_H / 2.0;
        let click = |app: &mut OndinApp, at: egui::Pos2, button: egui::PointerButton| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            pass(&ctx, app, vec![egui::Event::PointerMoved(at)]);
            pass(&ctx, app, vec![press(true)]);
            pass(&ctx, app, vec![press(false)]);
            pass(&ctx, app, Vec::new());
        };
        let menu = |app: &OndinApp| app.chart_data.as_ref().and_then(|d| d.menu.clone());
        let header = egui::pos2(grid.left() + GUTTER + COL_W - 10.0, y);
        click(&mut app, header, egui::PointerButton::Primary);
        assert!(menu(&app).is_none(), "a click selects");
        assert_eq!(app.chart_data.as_ref().map(|d| d.anchor), Some((0, 0)));
        click(&mut app, header, egui::PointerButton::Secondary);
        assert!(
            matches!(menu(&app), Some(Menu::Header(ColumnId(0), _))),
            "a right-click opens it"
        );
    }

    /// The window stays on its chart when the selection is cleared, and moves to
    /// another chart selected alone.
    /// flip: `chart_data_ui` closing on any other selection → fails at "stays open".
    #[test]
    fn the_window_stays_open_when_the_selection_moves_off() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        app.session.selection.clear();
        for _ in 0..2 {
            pass(&ctx, &mut app, Vec::new());
        }
        assert_eq!(
            app.chart_data.as_ref().map(|d| d.chart),
            Some(id),
            "stays open"
        );
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-3000.0, 0.0),
            Chart::sample(ChartKind::Line),
        );
        let other = *app.session.selection.ids().first().unwrap();
        assert_ne!(other, id, "fixture: a second chart");
        pass(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.chart_data.as_ref().map(|d| d.chart),
            Some(other),
            "moved"
        );
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

    /// A headless app with a new chart's data window open, a few passes in.
    fn opened(ctx: &egui::Context) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().unwrap();
        for _ in 0..3 {
            pass(ctx, &mut app, Vec::new());
        }
        (app, id)
    }

    /// Where the window drew the text `s`, from one pass's shapes.
    fn text_pos(ctx: &egui::Context, app: &mut OndinApp, s: &str) -> egui::Pos2 {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        fn find(shape: &egui::Shape, s: &str) -> Option<egui::Pos2> {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == s => {
                    Some(t.pos + t.galley.rect.center().to_vec2())
                }
                egui::Shape::Vec(v) => v.iter().find_map(|x| find(x, s)),
                _ => None,
            }
        }
        out.shapes
            .iter()
            .find_map(|c| find(&c.shape, s))
            .unwrap_or_else(|| panic!("{s} is drawn"))
    }

    /// The chart's name after *Chart data* sits on the title's baseline.
    /// flip: `settings::title_row` laying the note out as a centred label → fails
    /// at "one baseline".
    #[test]
    fn the_title_note_sits_on_the_titles_baseline() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let name = app.session.doc.get(id).unwrap().name().to_string();
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        // Every place `s` is drawn: the name shows on the inspector card too.
        fn baselines(shape: &egui::Shape, s: &str, out: &mut Vec<f32>) {
            match shape {
                egui::Shape::Text(t) if t.galley.text() == s => {
                    out.push(t.pos.y + ui::baseline_in(&t.galley));
                }
                egui::Shape::Vec(v) => v.iter().for_each(|x| baselines(x, s, out)),
                _ => {}
            }
        }
        let at = |s: &str| {
            let mut all = Vec::new();
            out.shapes
                .iter()
                .for_each(|c| baselines(&c.shape, s, &mut all));
            assert!(!all.is_empty(), "{s} is drawn");
            all
        };
        let title = at("Chart data")[0];
        let note = at(&name)
            .into_iter()
            .min_by(|a, b| (a - title).abs().total_cmp(&(b - title).abs()))
            .unwrap();
        assert!((title - note).abs() < 0.5, "one baseline: {title} {note}");
    }

    /// A cell typed into keeps its text where the grid drew it, a text cell and
    /// a number cell alike.
    /// flip: `grid` laying the edit as a `ui::text_field` → fails at "in place".
    #[test]
    fn a_cell_typed_into_keeps_its_text_in_place() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let t = table_of(&app, id);
        for at in [(0, 0), (0, 1)] {
            let text = t.rows[at.0][at.1].clone();
            let data = app.chart_data.as_mut().unwrap();
            (data.at, data.anchor, data.edit) = (at, at, None);
            let drawn = text_pos(&ctx, &mut app, &text);
            app.chart_data.as_mut().unwrap().edit = Typing::new(&t, at, text.clone());
            for _ in 0..3 {
                pass(&ctx, &mut app, Vec::new());
            }
            let typed = text_pos(&ctx, &mut app, &text);
            assert!(
                (drawn - typed).length() < 0.6,
                "{text} in place: {drawn:?} {typed:?}"
            );
        }
    }

    /// A click at `at`, moved onto first.
    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        pass(ctx, app, vec![egui::Event::PointerMoved(at)]);
        pass(ctx, app, vec![button(true)]);
        pass(ctx, app, vec![button(false)]);
        pass(ctx, app, Vec::new());
    }

    fn table_of(app: &OndinApp, id: NodeId) -> chart::Table {
        app.session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .table
            .clone()
    }

    /// *Add row* moves the cursor onto the new row, and a cell being typed
    /// into lands in its own cell in the same step.
    /// flip: `grid` clamping the cursor to the table as it was before the add → fails at "on the new row".
    #[test]
    fn add_row_moves_onto_it_and_keeps_the_typing() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let t = table_of(&app, id);
        let rows = t.rows.len();
        assert!(rows > 2, "the sample has rows to stand in");
        let data = app.chart_data.as_mut().unwrap();
        data.at = (1, 0);
        data.anchor = (1, 0);
        data.edit = Typing::new(&t, (1, 0), "typed".into());
        let at = text_pos(&ctx, &mut app, "Add row");
        click(&ctx, &mut app, at);
        let t = table_of(&app, id);
        assert_eq!(t.rows.len(), rows + 1, "a row was added");
        assert_eq!(t.rows[1][0], "typed", "the typing landed in its cell");
        assert_eq!(
            app.chart_data.as_ref().map(|d| d.at),
            Some((rows, 0)),
            "on the new row"
        );
    }

    /// A header menu open on a column an undo removes closes rather than
    /// reading past the table. Two defences each hold it alone (`grid`'s drop
    /// and `menus`' own `Menu::place` check), so no single flip bites.
    #[test]
    fn a_menu_on_a_column_undone_away_closes() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let cols = table_of(&app, id).columns.len();
        let at = text_pos(&ctx, &mut app, "Add column");
        click(&ctx, &mut app, at);
        assert_eq!(
            table_of(&app, id).columns.len(),
            cols + 1,
            "a column was added"
        );
        let added = table_of(&app, id).columns[cols].id;
        app.chart_data.as_mut().unwrap().menu = Some(Menu::Header(added, egui::pos2(400.0, 300.0)));
        app.session.undo();
        assert_eq!(table_of(&app, id).columns.len(), cols, "the add was undone");
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new());
        }
        assert!(
            app.chart_data.as_ref().is_some_and(|d| d.menu.is_none()),
            "the menu closed"
        );
    }

    /// *Insert row above* while a cell is being typed into writes the typing
    /// into its own row first, which then sits below the new one.
    /// flip: the row menu's insert built from `t.clone()` rather than
    /// `with_typed` → fails at "the typing kept its row".
    #[test]
    fn a_row_inserted_above_keeps_the_typing_in_its_row() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let t = table_of(&app, id);
        let data = app.chart_data.as_mut().unwrap();
        (data.at, data.anchor, data.focus) = ((1, 0), (1, 0), false);
        data.edit = Typing::new(&t, (1, 0), "typed".into());
        data.menu = Some(Menu::Row(1, t.rows[1].clone(), egui::pos2(500.0, 400.0)));
        // A new area is laid out unseen on its first pass.
        pass(&ctx, &mut app, Vec::new());
        let at = text_pos(&ctx, &mut app, "Insert row above");
        click(&ctx, &mut app, at);
        let next = table_of(&app, id);
        assert_eq!(next.rows.len(), t.rows.len() + 1, "a row was added");
        assert_eq!(next.rows[1][0], "", "the new row is empty");
        assert_eq!(next.rows[2][0], "typed", "the typing kept its row");
    }

    /// A table changed from outside while a cell is being typed into drops the
    /// typing: `Enter` then writes nowhere, not into the row that moved there.
    /// flip: `Typing::cell` skipping the check against the table it was started
    /// on → fails at "written nowhere".
    #[test]
    fn a_table_changed_under_the_typing_drops_it() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let t = table_of(&app, id);
        let data = app.chart_data.as_mut().unwrap();
        (data.at, data.anchor, data.focus) = ((1, 0), (1, 0), false);
        data.edit = Typing::new(&t, (1, 0), "typed".into());
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).cloned();
        let mut moved = t.clone();
        moved.rows.insert(0, vec![String::new(); t.columns.len()]);
        app.commit_table(id, &chart.unwrap(), moved);
        pass(&ctx, &mut app, Vec::new());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        let next = table_of(&app, id);
        assert!(
            next.rows.iter().all(|r| r[0] != "typed"),
            "written nowhere: {:?}",
            next.rows
        );
    }

    /// `id`'s table with a column put first and a row put on top, committed as
    /// an agent's edit would be; returns the table before.
    fn grown_from_outside(app: &mut OndinApp, id: NodeId) -> chart::Table {
        let t = table_of(app, id);
        let chart = app.session.doc.get(id).and_then(|n| n.chart()).cloned();
        let mut moved = t.clone();
        let fresh = t.columns.iter().map(|c| c.id.0 + 1).max().unwrap_or(0);
        moved.columns.insert(
            0,
            chart::Column {
                id: ColumnId(fresh),
                name: "New".into(),
            },
        );
        for r in &mut moved.rows {
            r.insert(0, String::new());
        }
        moved
            .rows
            .insert(0, vec![String::new(); moved.columns.len()]);
        app.commit_table(id, &chart.unwrap(), moved.clone());
        assert_eq!(table_of(app, id), moved, "fixture: the outside edit landed");
        t
    }

    /// A header menu still acts on its own column, and a row menu whose row
    /// moved closes, after a table changed from outside put a column and a row
    /// before them.
    /// flip: `Menu::place` reading a header menu's column at the place it was
    /// opened on → fails at "the right-clicked column went"; reading a row
    /// menu's row without its cells → fails at "the row menu closed".
    #[test]
    fn a_menu_follows_its_column_through_an_outside_edit() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let t = table_of(&app, id);
        let picked = t.columns[1].id;
        assert_eq!(picked, ColumnId(1), "fixture: ids match places");
        app.chart_data.as_mut().unwrap().menu =
            Some(Menu::Header(picked, egui::pos2(500.0, 400.0)));
        grown_from_outside(&mut app, id);
        pass(&ctx, &mut app, Vec::new());
        let at = text_pos(&ctx, &mut app, "Delete column");
        click(&ctx, &mut app, at);
        let ids: Vec<ColumnId> = table_of(&app, id).columns.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), t.columns.len(), "one column went");
        assert!(
            !ids.contains(&picked),
            "the right-clicked column went: {ids:?}"
        );

        let t = table_of(&app, id);
        app.chart_data.as_mut().unwrap().menu =
            Some(Menu::Row(1, t.rows[1].clone(), egui::pos2(500.0, 400.0)));
        pass(&ctx, &mut app, Vec::new());
        assert!(
            app.chart_data.as_ref().is_some_and(|d| d.menu.is_some()),
            "fixture: the row menu is open"
        );
        grown_from_outside(&mut app, id);
        pass(&ctx, &mut app, Vec::new());
        assert!(
            app.chart_data.as_ref().is_some_and(|d| d.menu.is_none()),
            "the row menu closed"
        );
    }

    /// A right-click on the gutter of the row being typed into commits the
    /// typing and leaves the row menu open on that row's new cells.
    /// flip: `commit_edit_text` leaving an open row menu's cells alone, or
    /// `menus` checking a row menu by `Menu::place` → fails at "the menu stays".
    #[test]
    fn a_row_menu_opened_on_the_typed_row_stays_open() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let t = table_of(&app, id);
        let data = app.chart_data.as_mut().unwrap();
        (data.at, data.anchor, data.focus) = ((1, 0), (1, 0), true);
        data.edit = Typing::new(&t, (1, 0), "typed".into());
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new());
        }
        let grid = ctx
            .read_response(egui::Id::new(GRID_ID))
            .expect("the grid is drawn")
            .rect;
        let at = egui::pos2(grid.left() + GUTTER / 2.0, grid.top() + ROW_H * 2.5);
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        pass(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        pass(&ctx, &mut app, vec![press(true)]);
        pass(&ctx, &mut app, vec![press(false)]);
        pass(&ctx, &mut app, Vec::new());
        assert_eq!(table_of(&app, id).rows[1][0], "typed", "the typing landed");
        let menu = app.chart_data.as_ref().and_then(|d| d.menu.clone());
        assert!(
            matches!(menu, Some(Menu::Row(1, ..))),
            "the menu stays: {menu:?}"
        );
    }

    /// A header being renamed writes its name onto its own column after a table
    /// changed from outside put another column before it.
    /// flip: `commit_rename` writing at the place the rename began → fails at
    /// "its own column".
    #[test]
    fn a_rename_follows_its_column_through_an_outside_edit() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        let picked = table_of(&app, id).columns[1].id;
        let data = app.chart_data.as_mut().unwrap();
        (data.rename, data.focus) = (Some((picked, "Income".into())), true);
        pass(&ctx, &mut app, Vec::new());
        grown_from_outside(&mut app, id);
        pass(&ctx, &mut app, Vec::new());
        assert!(
            app.chart_data.as_ref().is_some_and(|d| d.rename.is_some()),
            "fixture: still renaming"
        );
        pass(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        pass(&ctx, &mut app, Vec::new());
        let named: Vec<ColumnId> = table_of(&app, id)
            .columns
            .iter()
            .filter(|c| c.name == "Income")
            .map(|c| c.id)
            .collect();
        assert_eq!(named, [picked], "its own column");
    }

    /// `Escape` on an open header menu closes the menu and leaves the window.
    /// flip: `keys` answering `Escape` with *Close* while a menu is open →
    /// fails at "the window stays".
    #[test]
    fn escape_on_a_menu_closes_only_the_menu() {
        let ctx = egui::Context::default();
        let (mut app, _) = opened(&ctx);
        app.chart_data.as_mut().unwrap().menu =
            Some(Menu::Header(ColumnId(0), egui::pos2(500.0, 400.0)));
        pass(&ctx, &mut app, Vec::new());
        assert!(
            ctx.memory(|m| m.has_focus(egui::Id::new(GRID_ID))),
            "fixture: the grid has the keyboard"
        );
        pass(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(app.chart_data.is_some(), "the window stays");
        assert!(
            app.chart_data.as_ref().is_some_and(|d| d.menu.is_none()),
            "the menu closed"
        );
    }

    /// Two characters typed in one frame onto a selected cell both land.
    /// flip: `keys` starting the edit from the first `Text` event alone →
    /// fails at "both characters".
    #[test]
    fn keys_typed_in_one_frame_all_land() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text("4".into()), egui::Event::Text("2".into())],
        );
        pass(&ctx, &mut app, Vec::new());
        pass(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        assert_eq!(table_of(&app, id).rows[0][0], "42", "both characters");
    }

    /// `Tab` on a wide table moves the cursor, the card's focus ring leaving it
    /// to the grid, and scrolls it into view, as the arrows do.
    /// flip: `keys`' `Tab` arm leaving `reveal` unset → fails at "scrolled";
    /// `grid` not calling `ui::keep_tab` → fails at the cursor's cell.
    #[test]
    fn tab_scrolls_the_cursor_into_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut wide = Chart::sample(ChartKind::Column);
        let mut t = table(&[&["a"; 10], &["1"; 10]]);
        t.columns[0].name = "Category".into();
        wide.table = t;
        wide.mapping = chart::Mapping::guess(ChartKind::Column, &wide.table);
        app.insert_chart_at(ondin_core::kurbo::Point::new(-4000.0, 0.0), wide);
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new());
        }
        let left = |ctx: &egui::Context| {
            ctx.read_response(egui::Id::new(GRID_ID))
                .expect("the grid is drawn")
                .rect
                .left()
        };
        let before = left(&ctx);
        for _ in 0..8 {
            pass(&ctx, &mut app, vec![key(egui::Key::Tab)]);
        }
        pass(&ctx, &mut app, Vec::new());
        assert_eq!(app.chart_data.as_ref().map(|d| d.at), Some((0, 8)));
        assert!(
            left(&ctx) < before - 1.0,
            "scrolled: {before} → {}",
            left(&ctx)
        );
    }

    /// A cell being typed into lets go of the keyboard when it is taken
    /// elsewhere, and its typing lands.
    /// flip: `grid` asking the cell's focus back every pass → fails at "let go".
    #[test]
    fn a_cell_typed_into_lets_go_of_the_keyboard() {
        let ctx = egui::Context::default();
        let (mut app, id) = opened(&ctx);
        pass(&ctx, &mut app, vec![egui::Event::Text("7".into())]);
        pass(&ctx, &mut app, Vec::new());
        let cell = egui::Id::new((GRID_ID, "cell"));
        assert!(
            ctx.memory(|m| m.has_focus(cell)),
            "fixture: the cell has the keyboard"
        );
        ctx.memory_mut(|m| m.surrender_focus(cell));
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new());
        }
        assert!(!ctx.memory(|m| m.has_focus(cell)), "let go");
        assert_eq!(table_of(&app, id).rows[0][0], "7", "the typing landed");
    }

    /// The dim grid past a long, narrow table strokes only the cells in view.
    /// flip: the dim grid's rows running to the table's last row → fails at
    /// "only the view".
    #[test]
    fn the_dim_grid_draws_only_the_view() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut long = Chart::sample(ChartKind::Column);
        long.table = chart::Table {
            columns: vec![
                chart::Column {
                    id: ColumnId(0),
                    name: "Category".into(),
                },
                chart::Column {
                    id: ColumnId(1),
                    name: "Value".into(),
                },
            ],
            rows: (0..chart::MAX_ROWS)
                .map(|i| vec![format!("r{i}"), i.to_string()])
                .collect(),
        };
        long.mapping = chart::Mapping::guess(ChartKind::Column, &long.table);
        app.insert_chart_at(ondin_core::kurbo::Point::new(-4000.0, 0.0), long);
        let mut frame = eframe::Frame::_new_kittest();
        let mut shapes = 0;
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                ..Default::default()
            };
            let out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            shapes = out.shapes.len();
        }
        assert!(shapes < 10_000, "only the view: {shapes} shapes");
    }
}
