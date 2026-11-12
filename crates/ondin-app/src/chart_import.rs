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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// The file's stat when `text` was read, which the link stores, so a change
    /// while the box is open still reads as changed.
    stat: Option<(u64, u64)>,
}

/// A chart data file being read and parsed on a worker, and what its answer
/// is for. One at a time: a second read replaces the first, which stops
/// before its next parse and whose answer is dropped.
pub(crate) struct ChartRead {
    job: crate::job::Job<Result<Vec<Parsed>, String>>,
    then: Then,
    _stop: StopOnDrop,
}

/// Raises its flag when dropped: the read it belongs to was replaced or landed.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A file as the worker read it, under one header choice.
struct Parsed {
    text: String,
    read: table_io::Read,
    /// Its stat as read, for the link.
    stat: Option<(u64, u64)>,
}

/// What a read lands as.
enum Then {
    /// The import box for this chart; `linkable` when the file came with a path.
    Import {
        chart: NodeId,
        path: PathBuf,
        linkable: bool,
    },
    /// *Update chart data* for every chart linking `path`, read once under
    /// each header choice those charts made, in `headers`' order.
    Update {
        path: String,
        headers: Vec<Option<bool>>,
    },
    /// A new chart at this point, linked to `path` when `linkable` (a browser
    /// drop's path is only its name).
    Insert {
        at: ondin_core::kurbo::Point,
        path: PathBuf,
        linkable: bool,
    },
}

/// Where a read's bytes come from: a file, or a browser drop's bytes.
enum Source {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// Read `source` and parse it as `format` once per header choice in `headers`
/// (`None` guessed), a file by the tokens link's rules; `Err` says why not.
/// The stat is taken before the read, so a write landing during it leaves the
/// link reading as changed. `stop` ends it before the next parse.
fn read_source(
    source: Source,
    format: table_io::Format,
    headers: &[Option<bool>],
    stop: &AtomicBool,
) -> Result<Vec<Parsed>, String> {
    read_source_with(source, format, headers, stop, || {})
}

/// [`read_source`] with `between` run after the stat and the read, before the
/// parse: a test's writer racing the read.
fn read_source_with(
    source: Source,
    format: table_io::Format,
    headers: &[Option<bool>],
    stop: &AtomicBool,
    between: impl FnOnce(),
) -> Result<Vec<Parsed>, String> {
    let max = crate::tokens_import::MAX_READ;
    let (bytes, stat) = match source {
        Source::File(path) => {
            let stat = stat_of(&path);
            (crate::tokens_import::read_linked_bytes(&path, max)?, stat)
        }
        Source::Bytes(b) if b.len() as u64 > max => {
            return Err(format!("larger than {} MB", max / (1024 * 1024)));
        }
        Source::Bytes(b) => (b, None),
    };
    between();
    let text = decode(&bytes);
    drop(bytes);
    let mut out = Vec::with_capacity(headers.len());
    for header in headers {
        if stop.load(Ordering::Relaxed) {
            return Err("replaced by another read".into());
        }
        let read = table_io::read(&text, format, *header).map_err(|e| e.to_string())?;
        out.push(Parsed {
            text: text.clone(),
            read,
            stat,
        });
    }
    Ok(out)
}

/// A data file's bytes as text: UTF-8, UTF-16 by its byte-order mark, else
/// Windows-1252, the code page Excel writes *CSV (Comma delimited)* in, which
/// covers Latin-1 too.
fn decode(bytes: &[u8]) -> String {
    let utf16 = |rest: &[u8], be: bool| {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| match be {
                true => u16::from_be_bytes(*p),
                false => u16::from_le_bytes(*p),
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFF, 0xFE, rest @ ..] => return utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => return utf16(rest, true),
        _ => {}
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    /// 0x80–0x9F in Windows-1252; the five it leaves undefined pass through.
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž',
        '\u{8F}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}',
        'ž', 'Ÿ',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
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
    /// when the file came without one, once a worker has read it. A file that
    /// does not read is refused on the status line; the same file asked for
    /// again while it is being read is not read twice.
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
        if self.reading(|t| {
            matches!(t, Then::Import { chart, path: p, linkable: l }
                if *chart == id && *p == path && *l == linkable)
        }) {
            return;
        }
        let source = match bytes {
            Some(b) => Source::Bytes(b),
            None => Source::File(path.clone()),
        };
        let then = Then::Import {
            chart: id,
            path,
            linkable,
        };
        self.start_chart_read(&name, source, format, vec![None], then);
    }

    /// Read `source` as `format` on a worker, once per header choice in
    /// `headers`, for `then`; the status line says *Reading sales.csv…* until
    /// it lands. A read already pending is replaced, and stops.
    fn start_chart_read(
        &mut self,
        name: &str,
        source: Source,
        format: table_io::Format,
        headers: Vec<Option<bool>>,
        then: Then,
    ) {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let job = crate::job::Job::spawn("ondin-chart-read", &self.egui_ctx, move || {
            read_source(source, format, &headers, &flag)
        });
        self.session.info(format!("Reading {name}\u{2026}"));
        self.chart_read = Some(ChartRead {
            job,
            then,
            _stop: StopOnDrop(stop),
        });
    }

    /// Whether a read is pending whose landing `same` recognises.
    fn reading(&self, same: impl Fn(&Then) -> bool) -> bool {
        self.chart_read.as_ref().is_some_and(|r| same(&r.then))
    }

    /// The import box for chart `id` on a file read as `parsed`.
    fn open_import_box(&mut self, id: NodeId, path: PathBuf, linkable: bool, parsed: Parsed) {
        let Some(format) = path
            .file_name()
            .and_then(|n| table_io::Format::of_name(&n.to_string_lossy()))
        else {
            return;
        };
        self.chart_import = Some(ChartImport {
            chart: id,
            path,
            linkable,
            text: parsed.text,
            format,
            header: parsed.read.header,
            read: parsed.read,
            link: linkable,
            stat: parsed.stat,
        });
    }

    /// Land the chart data read on a worker, once it has: open the import box,
    /// update the linked charts, or insert the dropped chart, by what it was for.
    /// A refusal is said on the status line. Held while a gesture is live or a
    /// modal is up, as `poll_imports` is: a landing selects and opens things.
    pub(crate) fn poll_chart_read(&mut self) {
        if self.gesture_live() || self.modal_is_up() {
            return;
        }
        let Some(pending) = &self.chart_read else {
            return;
        };
        let answer = match pending.job.poll() {
            Ok(None) => return,
            Ok(Some(a)) => a,
            Err(()) => Err("the read stopped".to_string()),
        };
        let Some(ChartRead { then, .. }) = self.chart_read.take() else {
            return;
        };
        self.land_chart_read(then, answer);
    }

    /// Block until the pending read lands, then land it (tests only).
    #[cfg(test)]
    pub(crate) fn settle_chart_read(&mut self) {
        if let Some(ChartRead { job, then, _stop }) = self.chart_read.take() {
            let answer = job
                .wait()
                .unwrap_or_else(|| Err("the read stopped".to_string()));
            self.land_chart_read(then, answer);
        }
    }

    fn land_chart_read(&mut self, then: Then, answer: Result<Vec<Parsed>, String>) {
        // *Reading sales.csv…* is over; a landing that reports says so itself.
        self.session.info(String::new());
        match then {
            Then::Import {
                chart,
                path,
                linkable,
            } => {
                let name = file_name(&path.to_string_lossy());
                match answer.map(|mut all| all.pop()) {
                    Ok(Some(parsed))
                        if self
                            .session
                            .doc
                            .get(chart)
                            .is_some_and(|n| n.chart().is_some()) =>
                    {
                        self.open_import_box(chart, path, linkable, parsed);
                    }
                    Ok(_) => {}
                    Err(why) => self.session.fail(format!("Cannot import {name}: {why}")),
                }
            }
            Then::Update { path, headers } => match answer {
                Ok(parsed) => {
                    let by_header = headers.into_iter().zip(parsed).collect();
                    self.replace_linked_tables(&path, by_header);
                }
                Err(why) => self
                    .session
                    .fail(format!("Cannot update from {}: {why}", file_name(&path))),
            },
            Then::Insert { at, path, linkable } => match answer.map(|mut all| all.pop()) {
                Ok(Some(parsed)) => {
                    let link = linkable.then_some(path.as_path());
                    self.insert_chart_from(at, link, parsed);
                }
                Ok(None) => {}
                Err(why) => {
                    let name = file_name(&path.to_string_lossy());
                    self.session.fail(format!("Cannot import {name}: {why}"));
                }
            },
        }
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
            // The box's `Enter` is spent here, not read again by the data
            // window drawn after it this frame.
            ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let link = (t.link && t.linkable).then(|| {
                let (modified, size) = t.stat.unwrap_or_default();
                DataLink {
                    path: t.path.to_string_lossy().into_owned(),
                    size,
                    modified,
                    header: Some(t.header),
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

    /// The header choice of chart `id`'s link, `None` when guessed.
    fn link_header(&self, id: NodeId) -> Option<bool> {
        self.session.doc.get(id)?.chart()?.source.as_ref()?.header
    }

    /// *Update chart data*: read `path` and replace every chart linked to it, one
    /// undo step. The file's contents enter the document only here, on a click.
    /// A click while that read is pending reads nothing more.
    pub(crate) fn update_linked_charts(&mut self, path: &str) {
        let name = file_name(path);
        let Some(format) = table_io::Format::of_name(&name) else {
            return;
        };
        if self.reading(|t| matches!(t, Then::Update { path: p, .. } if p == path)) {
            return;
        }
        // Read once, parsed under each header choice the linking charts made.
        let mut headers: Vec<Option<bool>> = Vec::new();
        let ids = self
            .changed_links()
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, ids)| ids)
            .unwrap_or_default();
        for id in ids {
            let header = self.link_header(id);
            if !headers.contains(&header) {
                headers.push(header);
            }
        }
        if headers.is_empty() {
            headers.push(None);
        }
        let then = Then::Update {
            path: path.to_string(),
            headers: headers.clone(),
        };
        let source = Source::File(PathBuf::from(path));
        self.start_chart_read(&name, source, format, headers, then);
    }

    /// Replace the table of every chart whose link to `path` changed with the
    /// read under its own header choice, one undo step, *Update chart data*.
    fn replace_linked_tables(&mut self, path: &str, by_header: Vec<(Option<bool>, Parsed)>) {
        let name = file_name(path);
        let ids: Vec<NodeId> = self
            .changed_links()
            .into_iter()
            .find(|(p, _)| p == path)
            .map(|(_, ids)| ids)
            .unwrap_or_default();
        let ops: Vec<Operation> = ids
            .iter()
            .filter_map(|id| {
                let header = self.link_header(*id);
                let (_, parsed) = by_header.iter().find(|(h, _)| *h == header)?;
                let chart = self.session.doc.get(*id)?.chart()?;
                let (modified, size) = parsed.stat.unwrap_or_default();
                let next = Chart {
                    source: Some(DataLink {
                        path: path.to_string(),
                        size,
                        modified,
                        header,
                    }),
                    ..chart.with_table(parsed.read.table.clone())
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
        if target.is_none() && format == table_io::Format::Json {
            return false;
        }
        // A browser drop's bytes, refused past the read limit before they are
        // copied, and parsed on the worker as a file is.
        let max = crate::tokens_import::MAX_READ;
        let bytes = match (&file.path, &file.bytes) {
            (Some(_), _) => None,
            (None, Some(b)) if b.len() as u64 > max => {
                self.session.fail(format!(
                    "Cannot import {name}: larger than {} MB",
                    max / (1024 * 1024)
                ));
                return true;
            }
            (None, Some(b)) => Some(b.to_vec()),
            (None, None) => {
                self.session
                    .fail(format!("Cannot import {name}: the drop brought no file"));
                return true;
            }
        };
        if let Some(id) = target {
            self.session.selection.set_one(id);
            self.open_chart_import(id, path, bytes);
            return true;
        }
        let p = at.unwrap_or_else(|| self.viewport_centre());
        let linkable = bytes.is_none();
        let source = match bytes {
            Some(b) => Source::Bytes(b),
            None => Source::File(path.clone()),
        };
        let then = Then::Insert {
            at: p,
            path,
            linkable,
        };
        self.start_chart_read(name, source, format, vec![None], then);
        true
    }

    /// A new chart at `at` from a table file, its kind read off the columns,
    /// linked to `path` when the file came with one.
    fn insert_chart_from(
        &mut self,
        at: ondin_core::kurbo::Point,
        path: Option<&Path>,
        parsed: Parsed,
    ) {
        let read = parsed.read;
        let kind = kind_for(&read.table);
        let mut c = Chart::sample(kind);
        c.mapping = chart::Mapping::guess(kind, &read.table);
        c.table = read.table;
        c.source = path.map(|p| {
            let (modified, size) = parsed.stat.unwrap_or_default();
            DataLink {
                path: p.to_string_lossy().into_owned(),
                size,
                modified,
                header: None,
            }
        });
        self.insert_chart_at(at, c);
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
        assert!(
            app.chart_import.is_none(),
            "the file is read off the UI thread"
        );
        app.settle_chart_read();
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
        app.settle_chart_read();
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
        app.settle_chart_read();
        assert!(app.chart_import.is_none(), "a share is never read");
    }

    /// The import box holds the keymap off: `Delete` while it is up leaves the
    /// selected chart where it is.
    /// flip: `modal_is_up` leaving out `chart_import` → fails at "the chart stays".
    #[test]
    fn delete_under_the_import_box_keeps_the_chart() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        app.chart_data = None;
        let id = *app.session.selection.ids().first().expect("the chart");
        let path = temp_csv("modal", "Category,Revenue\nQ1,10\n");
        app.open_chart_import(id, path.clone(), None);
        app.settle_chart_read();
        assert!(app.chart_import.is_some(), "the box opened");
        pass(&ctx, &mut app, Vec::new());
        let delete = egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        pass(&ctx, &mut app, vec![delete]);
        pass(&ctx, &mut app, Vec::new());
        assert!(app.session.doc.get(id).is_some(), "the chart stays");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// *Update* reads the file the way the import did: a header turned off in
    /// the box stays off, rather than guessed again.
    /// flip: `update_linked_charts` reading with the header guessed → fails at "the first row is data".
    #[test]
    fn update_keeps_the_header_choice_of_the_import() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().expect("the chart");
        let path = temp_csv("header", "Category,Revenue\nQ1,10\n");
        app.open_chart_import(id, path.clone(), None);
        app.settle_chart_read();
        let t = app.chart_import.as_mut().expect("the box opened");
        assert!(t.header, "the guess takes the first row as the header");
        t.header = false;
        t.read = table_io::read(&t.text, t.format, Some(false)).unwrap();
        pass(&ctx, &mut app, Vec::new());
        pass(&ctx, &mut app, vec![enter()]);
        let rows = |app: &OndinApp| {
            app.session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .unwrap()
                .table
                .rows
                .len()
        };
        assert_eq!(rows(&app), 2, "imported with the first row as data");
        std::fs::write(&path, "Category,Revenue\nQ1,10\nQ2,12\n").unwrap();
        app.chart_watch.next = 0.0;
        app.watch_chart_links(&ctx);
        assert_eq!(app.chart_link_state(id), Some(LinkState::Changed));
        let link = app
            .session
            .doc
            .get(id)
            .and_then(|n| n.chart())
            .unwrap()
            .source
            .clone()
            .unwrap();
        app.update_linked_charts(&link.path);
        app.settle_chart_read();
        assert_eq!(rows(&app), 3, "the first row is data");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Excel's *CSV (Comma delimited)*, written in Windows-1252, reads with its
    /// accents; UTF-16 by its byte-order mark too.
    /// flip: `decode` taking non-UTF-8 bytes lossily → fails at "Café".
    #[test]
    fn an_ansi_csv_reads_with_its_accents() {
        let stop = AtomicBool::new(false);
        let csv = table_io::Format::Csv;
        let dir = temp_csv("ansi", "");
        let path = dir.with_file_name("ansi.csv");
        std::fs::write(&path, b"name,v\nCaf\xe9 \x80,1\n").unwrap();
        let read = read_source(Source::File(path.clone()), csv, &[None], &stop).unwrap();
        assert_eq!(read[0].read.table.columns.len(), 2);
        assert_eq!(read[0].read.table.rows[0][0], "Café €");
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("n,v\nÅ,2\n".encode_utf16().flat_map(u16::to_le_bytes));
        let read = read_source(Source::Bytes(utf16), csv, &[None], &stop).unwrap();
        assert_eq!(read[0].read.table.rows[0][0], "Å");
        let gone = read_source(
            Source::File(dir.with_file_name("gone.csv")),
            csv,
            &[None],
            &stop,
        );
        assert_eq!(
            gone.err().as_deref(),
            Some("file not found"),
            "the reason said"
        );
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }

    /// The stat a read stores is taken before the read, so a write landing
    /// during it leaves the link reading as changed.
    /// flip: `read_source_with` taking the stat after the read → fails at "the old stat".
    #[test]
    fn a_write_during_the_read_leaves_the_link_changed() {
        let path = temp_csv("race", "Category,Revenue\nQ1,10\n");
        let before = stat_of(&path);
        let stop = AtomicBool::new(false);
        let source = Source::File(path.clone());
        let read = read_source_with(source, table_io::Format::Csv, &[None], &stop, || {
            std::fs::write(&path, "Category,Revenue\nQ1,10\nQ2,12\nQ3,14\n").unwrap();
        })
        .unwrap();
        assert_ne!(
            before,
            stat_of(&path),
            "fixture: the write changed the stat"
        );
        assert_eq!(read[0].stat, before, "the old stat");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Two charts linking one file with opposite header choices each read it
    /// their own way on *Update*.
    /// flip: `update_linked_charts` reading under the first chart's header
    /// alone → fails at "B keeps its first row".
    #[test]
    fn update_reads_each_charts_header_choice() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let path = temp_csv("two", "Category,Revenue\nQ1,10\n");
        let mut ids = Vec::new();
        for (x, header) in [(-4000.0, true), (-3000.0, false)] {
            app.insert_chart_at(
                ondin_core::kurbo::Point::new(x, 0.0),
                Chart::sample(ChartKind::Column),
            );
            let id = *app.session.selection.ids().first().unwrap();
            app.open_chart_import(id, path.clone(), None);
            app.settle_chart_read();
            let t = app.chart_import.as_mut().expect("the box opened");
            t.header = header;
            t.read = table_io::read(&t.text, t.format, Some(header)).unwrap();
            pass(&ctx, &mut app, Vec::new());
            pass(&ctx, &mut app, vec![enter()]);
            ids.push(id);
        }
        let rows = |app: &OndinApp, id: NodeId| {
            app.session
                .doc
                .get(id)
                .and_then(|n| n.chart())
                .unwrap()
                .table
                .rows
                .len()
        };
        assert_eq!((rows(&app, ids[0]), rows(&app, ids[1])), (1, 2), "fixture");
        std::fs::write(&path, "Category,Revenue\nQ1,10\nQ2,12\n").unwrap();
        app.chart_watch.next = 0.0;
        app.watch_chart_links(&ctx);
        app.update_linked_charts(&path.to_string_lossy());
        app.settle_chart_read();
        assert_eq!(rows(&app, ids[0]), 2, "A reads a header");
        assert_eq!(rows(&app, ids[1]), 3, "B keeps its first row");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A second *Update* while the first read is pending starts no second read.
    /// flip: `update_linked_charts` without its `reading` check → fails at "not replaced".
    #[test]
    fn a_repeated_update_reads_once() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        // Built by `join`, so every platform reads `sales.csv` as its name.
        let path = std::env::temp_dir().join("nowhere").join("sales.csv");
        let path = path.to_string_lossy();
        app.update_linked_charts(&path);
        let first = Arc::clone(&app.chart_read.as_ref().expect("a read")._stop.0);
        app.update_linked_charts(&path);
        assert!(!first.load(Ordering::Relaxed), "not replaced");
        assert_eq!(app.session.status().text, "Reading sales.csv\u{2026}");
    }

    /// A read landing while a drag is live waits for it to end.
    /// flip: `poll_chart_read` without its gesture hold → fails at "held".
    #[test]
    fn a_read_waits_out_a_drag() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let read = table_io::read("x,y\n1,2\n", table_io::Format::Csv, None).unwrap();
        let parsed = Parsed {
            text: String::new(),
            read,
            stat: None,
        };
        app.chart_read = Some(ChartRead {
            job: crate::job::Job::done(Ok(vec![parsed])),
            then: Then::Insert {
                at: ondin_core::kurbo::Point::new(-4000.0, 0.0),
                path: PathBuf::from("drop.csv"),
                linkable: false,
            },
            _stop: StopOnDrop(Arc::new(AtomicBool::new(false))),
        });
        app.drag = crate::preview::Drag::Move {
            anchor: ondin_core::kurbo::Point::ZERO,
        };
        app.poll_chart_read();
        assert!(app.chart_read.is_some(), "held");
        app.drag = crate::preview::Drag::None;
        app.poll_chart_read();
        assert!(app.chart_read.is_none(), "landed once the drag ended");
        assert!(app.chart_data.is_some(), "the new chart's window opened");
    }

    /// A browser drop's bytes are read on the worker, as a file is, and refused
    /// past the read limit.
    /// flip: `open_chart_import` parsing the bytes in the frame → fails at "on the worker".
    #[test]
    fn a_browser_drop_is_read_off_the_frame() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().unwrap();
        let bytes = b"Category,Revenue\nQ1,10\n".to_vec();
        app.open_chart_import(id, PathBuf::from("drop.csv"), Some(bytes));
        assert!(app.chart_import.is_none(), "on the worker");
        app.settle_chart_read();
        let t = app.chart_import.as_ref().expect("the box opened");
        assert!(!t.linkable, "a browser drop has no path to link");
        app.chart_import = None;
        let big = egui::DroppedFile {
            name: "big.csv".into(),
            bytes: Some(vec![b'a'; crate::tokens_import::MAX_READ as usize + 1].into()),
            ..Default::default()
        };
        assert!(app.drop_chart_data(&big, "big.csv", None));
        assert!(app.chart_read.is_none(), "refused before reading");
        assert!(app.session.status().text.contains("larger than 16 MB"));
    }

    /// A link stores the file's stat as it was read, so a change made while
    /// the box was open reads as changed once the import lands.
    /// flip: the box's *Replace* taking the stat from the file at the click → fails at "the change is seen".
    #[test]
    fn a_file_changed_under_the_import_box_reads_as_changed() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app.insert_chart_at(
            ondin_core::kurbo::Point::new(-4000.0, 0.0),
            Chart::sample(ChartKind::Column),
        );
        let id = *app.session.selection.ids().first().expect("the chart");
        let path = temp_csv("stat", "Category,Revenue\nQ1,10\n");
        app.open_chart_import(id, path.clone(), None);
        app.settle_chart_read();
        assert!(app.chart_import.is_some(), "the box opened");
        std::fs::write(&path, "Category,Revenue\nQ1,10\nQ2,12\nQ3,14\n").unwrap();
        pass(&ctx, &mut app, Vec::new());
        pass(&ctx, &mut app, vec![enter()]);
        assert!(app.chart_import.is_none(), "Enter replaced the data");
        app.chart_watch.next = 0.0;
        app.watch_chart_links(&ctx);
        assert_eq!(
            app.chart_link_state(id),
            Some(LinkState::Changed),
            "the change is seen"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
