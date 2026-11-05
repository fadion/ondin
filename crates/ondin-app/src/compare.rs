//! *Compare with a version*: the open document drawn as it is now, read-only,
//! with a mark over every layer that differs from a pinned version, and a card in
//! the inspector's place listing the changes as `ondin diff` reports them
//! (`ondin_core::diff`, matched by id, so a renamed or moved layer is one change).
//! A bar over the canvas's top switches between Then (the version), Now and Swipe
//! (the two split at a handle), steps through the changes and leaves. Two versions
//! can be compared with each other too; the newer then stands for Now.
//!
//! **A mode, not a document**: nothing is written. The compare lives on the
//! document's session, which refuses commits, undo and redo while it is open;
//! `dispatch` admits only the verbs that look ([`admits`]), and the canvas only
//! pans and picks a mark.
//!
//! **The topmost changed layer is marked**: a layer under another listed, changed
//! one is folded into that row, its changes named under it and counted in the
//! mark's tag.

use crate::app::OndinApp;
use crate::canvas::CanvasRenderer;
use crate::input::{Action, ViewSwitch};
use crate::session::InkLog;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::diff::{self, Change, Status};
use ondin_core::kurbo::Rect;
use ondin_core::{Document, NodeId, Resolved};
use ondin_render::RenderOverrides;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What a refused edit says while a compare is open.
pub(crate) const READ_ONLY: &str = "Comparing with a version — close it, or press Esc, to edit";

/// The marks' hues and the ink on their tags. Told apart by more than hue: added
/// is solid with `+`, removed dashed with `−`, changed solid with `~`.
pub(crate) const ADDED: egui::Color32 = egui::Color32::from_rgb(0x4f, 0x8f, 0xe6);
pub(crate) const REMOVED: egui::Color32 = egui::Color32::from_rgb(0xe8, 0x96, 0x4a);
pub(crate) const CHANGED: egui::Color32 = egui::Color32::from_rgb(0xc9, 0xb4, 0x58);
const ADDED_INK: egui::Color32 = egui::Color32::from_rgb(0x0b, 0x12, 0x20);
const REMOVED_INK: egui::Color32 = egui::Color32::from_rgb(0x1d, 0x12, 0x06);
pub(crate) const CHANGED_INK: egui::Color32 = egui::Color32::from_rgb(0x17, 0x14, 0x0a);

/// How far outside a layer's box its mark is drawn, in screen points.
const MARK_OUTSET: f32 = 3.0;
/// The narrowest a mark's tag is cut to, over a layer narrower than it.
const TAG_MIN_W: f32 = 48.0;
/// How near the Swipe handle a press must land to drag it.
const HANDLE_REACH: f32 = 12.0;
/// The changes card's row inset and the indent of a row's value lines.
const ROW_PAD: f32 = 8.0;
const LINE_INDENT: f32 = 23.0;
/// How far an expanded row's folded layer's name and values sit past its tag.
const FOLDED_INDENT: f32 = 17.0;
/// A row's value lines past this many are summed up in one more.
const LINES_SHOWN: usize = 4;

/// Which state the canvas draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Side {
    Then,
    #[default]
    Now,
    /// Then left of a handle, Now right of it.
    Swipe,
}

/// One page's heading in the card.
#[derive(Debug)]
pub(crate) struct Group {
    pub(crate) name: String,
    /// The page, in either document; `None` for the kit vault.
    pub(crate) page: Option<NodeId>,
    /// Only Then has it, so its heading is dimmed.
    pub(crate) removed: bool,
    /// How many layers changed on it.
    pub(crate) layers: usize,
}

/// One change the card lists and the canvas marks.
#[derive(Debug)]
pub(crate) struct Row {
    pub(crate) id: NodeId,
    /// Its page's index in [`Compare::groups`].
    pub(crate) group: usize,
    /// The page it is drawn on, in the document holding it; `None` when the
    /// canvas cannot show it.
    pub(crate) page: Option<NodeId>,
    pub(crate) status: Status,
    /// A page's own change (its name, its background), which has no mark.
    pub(crate) is_page: bool,
    /// The names above it, each followed by ` / `.
    pub(crate) trail: String,
    pub(crate) name: String,
    /// The changed layers it stands for, itself included.
    pub(crate) layers: usize,
    /// For an added or removed layer, how many came or went inside it.
    pub(crate) inside: usize,
    /// Its own changes; the folded layers' are in `folded`.
    pub(crate) lines: Vec<Change>,
    /// The listed layers under it, in the diff's order.
    pub(crate) folded: Vec<Folded>,
    /// Its box in the open document and in the version.
    pub(crate) now: Option<Rect>,
    pub(crate) then: Option<Rect>,
}

/// A listed layer folded into the row of a listed layer above it.
#[derive(Debug)]
pub(crate) struct Folded {
    pub(crate) id: NodeId,
    pub(crate) status: Status,
    pub(crate) name: String,
    pub(crate) lines: Vec<Change>,
}

impl Row {
    /// Its changes and its folded layers' as one list, each folded change named
    /// *Child · what*, or *Child added* / *removed*: what a folded row shows.
    pub(crate) fn all_lines(&self) -> Vec<Change> {
        let mut out = self.lines.clone();
        for f in &self.folded {
            match f.status {
                Status::Changed => out.extend(f.lines.iter().map(|c| Change {
                    what: format!("{} · {}", f.name, c.what),
                    ..c.clone()
                })),
                Status::Added | Status::Removed => out.push(Change {
                    what: format!("{} {}", f.name, f.status_word()),
                    before: None,
                    after: None,
                }),
            }
        }
        out
    }

    /// Moved or resized: in both, its box differing by `diff::EPSILON` or more.
    pub(crate) fn moved(&self) -> bool {
        match (self.now, self.then) {
            (Some(a), Some(b)) => [a.x0 - b.x0, a.y0 - b.y0, a.x1 - b.x1, a.y1 - b.y1]
                .iter()
                .any(|d| d.abs() >= diff::EPSILON),
            _ => false,
        }
    }

    /// Where its mark goes in world space: where it was, for a removed layer.
    fn shown_at(&self) -> Option<Rect> {
        match self.status {
            Status::Removed => self.then,
            _ => self.now,
        }
    }
}

impl Folded {
    fn status_word(&self) -> &'static str {
        match self.status {
            Status::Added => "added",
            Status::Removed => "removed",
            Status::Changed => "changed",
        }
    }
}

/// A pinned version as the compare is asked for it: its file and what the bar
/// names it by.
#[derive(Clone, Debug)]
pub(crate) struct Pick {
    pub(crate) path: PathBuf,
    /// The version's name, else when it was pinned.
    pub(crate) title: String,
    /// When it was pinned, beside a name.
    pub(crate) when: Option<String>,
}

/// The newer of two versions compared with each other, which stands for Now.
pub(crate) struct Newer {
    pub(crate) pick: Pick,
    pub(crate) doc: Document,
    pub(crate) res: Resolved,
}

/// An open compare (`EditorSession::compare`).
pub(crate) struct Compare {
    /// The version, or the older of two.
    pub(crate) pick: Pick,
    pub(crate) then: Document,
    then_res: Resolved,
    /// The newer version when two are compared; `None` compares with the open
    /// document.
    pub(crate) newer: Option<Newer>,
    /// The stamps the version's canvas keys its texture on; the version never
    /// changes, so one serves for good.
    stamp: u64,
    overrides: RenderOverrides,
    ink: InkLog,
    pub(crate) groups: Vec<Group>,
    pub(crate) rows: Vec<Row>,
    /// Variables, modes, styles, guides and the like, which no page holds.
    pub(crate) off_canvas: Vec<Change>,
    /// The row the canvas rings and the card fills.
    pub(crate) current: Option<usize>,
    /// The rows whose folded layers the card lists apart.
    pub(crate) expanded: HashSet<usize>,
    pub(crate) side: Side,
    /// Where Swipe splits the canvas, as a fraction of its width.
    pub(crate) split: f32,
    /// Where the split stood when its handle's drag began, while it is dragged; a
    /// right-click puts it back there (`OndinApp::cancel_gesture`).
    pub(crate) split_from: Option<f32>,
    /// Whether the marks are drawn: the bar's *Overlay*, or `O`. A click on a card
    /// row turns it back on; stepping leaves it, so Then and Now can be flicked
    /// between bare.
    pub(crate) overlay: bool,
    /// A row stepped to on another page, revealed once its page is showing.
    reveal: Option<usize>,
    /// The page the compare opened on and its selection, given back when it closes
    /// there; the document cannot change meanwhile, so it still holds.
    selection: (NodeId, crate::session::Selection),
    /// The pages the open document lacks, each with its status, listed dimmed in
    /// the Pages section so the canvas can show them.
    pub(crate) elsewhere: Vec<(NodeId, Status)>,
    /// One of them while the canvas shows it, and the camera of the open
    /// document's page, given back on leaving it. `EditorSession::page` stays a
    /// page of the open document all the while.
    pub(crate) away: Option<(NodeId, crate::view::Camera)>,
}

impl Compare {
    /// What Now draws: the newer version, else the open document `open`.
    pub(crate) fn now_doc<'a>(&'a self, open: &'a Document) -> &'a Document {
        self.newer.as_ref().map_or(open, |n| &n.doc)
    }

    /// The compared document holding `page`, one the open document lacks: the
    /// newer version when it has it, else the version.
    pub(crate) fn page_doc(&self, page: NodeId) -> &Document {
        match &self.newer {
            Some(n) if n.doc.is_page(page) => &n.doc,
            _ => &self.then,
        }
    }

    /// `page`'s bounds in [`Self::page_doc`].
    pub(crate) fn page_bounds(&self, page: NodeId) -> Option<Rect> {
        match &self.newer {
            Some(n) if n.doc.is_page(page) => n.res.world_bounds(page),
            _ => self.then_res.world_bounds(page),
        }
    }

    /// What a page the open document lacks is, said over its Pages row.
    pub(crate) fn elsewhere_tip(&self, status: Status) -> &'static str {
        match (self.newer.is_some(), status) {
            (false, _) => "Only in the version",
            (true, Status::Removed) => "Only in the older version",
            (true, Status::Added) => "Only in the newer version",
            (true, Status::Changed) => "In both versions, not in the document",
        }
    }

    /// How many layers changed on `page` of the open document.
    pub(crate) fn changed_on(&self, page: NodeId) -> usize {
        self.groups
            .iter()
            .filter(|g| g.page == Some(page))
            .map(|g| g.layers)
            .sum()
    }
}

/// The card's groups and rows for what changed from `then` to `now`, and what
/// changed outside every page. `then_res` and `now_res` give the boxes.
pub(crate) fn rows(
    then: &Document,
    now: &Document,
    then_res: &Resolved,
    now_res: &Resolved,
) -> (Vec<Group>, Vec<Row>, Vec<Change>) {
    let d = diff::diff(then, now);
    let mut groups = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    for page in &d.pages {
        let Some(page_id) = NodeId::from_wire(&page.id) else {
            continue;
        };
        let group = groups.len();
        let shown = (now.is_page(page_id) || then.is_page(page_id)).then_some(page_id);
        groups.push(Group {
            name: page.name.clone(),
            page: shown,
            removed: page.status == Status::Removed,
            layers: 0,
        });
        if page.status != Status::Changed || !page.changes.is_empty() {
            rows.push(Row {
                id: page_id,
                group,
                page: shown,
                status: page.status,
                is_page: true,
                trail: String::new(),
                name: page.name.clone(),
                layers: 0,
                inside: page.inside,
                lines: page.changes.clone(),
                folded: Vec::new(),
                now: None,
                then: None,
            });
        }
        let tops = topmost(&page.layers, then, now);
        // The row each topmost layer went into, by its index in the page's list.
        let mut row_of: HashMap<usize, usize> = HashMap::new();
        for (i, layer) in page.layers.iter().enumerate() {
            let (Some(id), Some(top)) = (NodeId::from_wire(&layer.id), tops[i]) else {
                continue;
            };
            groups[group].layers += 1;
            let name = layer.trail.last().cloned().unwrap_or_default();
            if top != i {
                let row = &mut rows[row_of[&top]];
                row.layers += 1;
                row.folded.push(Folded {
                    id,
                    status: layer.status,
                    name,
                    lines: layer.changes.clone(),
                });
                continue;
            }
            let page_of = match layer.status {
                Status::Removed => then.page_of(id).filter(|p| then.is_page(*p)),
                _ => now.page_of(id).filter(|p| now.is_page(*p)),
            };
            let above = &layer.trail[..layer.trail.len().saturating_sub(1)];
            row_of.insert(i, rows.len());
            rows.push(Row {
                id,
                group,
                page: page_of,
                status: layer.status,
                is_page: false,
                trail: above.iter().map(|n| format!("{n} / ")).collect(),
                name,
                layers: 1,
                inside: layer.inside,
                lines: layer.changes.clone(),
                folded: Vec::new(),
                now: now.contains(id).then(|| now_res.world_bounds(id)).flatten(),
                then: then
                    .contains(id)
                    .then(|| then_res.world_bounds(id))
                    .flatten(),
            });
        }
    }
    (groups, rows, d.document)
}

/// For each of a page's listed `layers`, the index of the listed layer whose row it
/// folds into: its nearest listed ancestor's, or its own when none is listed or
/// that row is an added or removed one, whose `inside` counts only the layers that
/// came or went with it. `None` for an id that does not parse.
pub(crate) fn topmost(
    layers: &[diff::LayerDiff],
    then: &Document,
    now: &Document,
) -> Vec<Option<usize>> {
    let mut top_of: HashMap<NodeId, usize> = HashMap::new();
    layers
        .iter()
        .enumerate()
        .map(|(i, layer)| {
            let id = NodeId::from_wire(&layer.id)?;
            let top = ancestors(id, then, now)
                .find_map(|a| top_of.get(&a).copied())
                .filter(|t| layers[*t].status == Status::Changed)
                .unwrap_or(i);
            top_of.insert(id, top);
            Some(top)
        })
        .collect()
}

/// The compared pages the open document `open` lacks: `now`'s in its order, added
/// or (when `then` has it too) changed, then `then`'s that `now` lacks, removed.
fn elsewhere(then: &Document, now: &Document, open: &Document) -> Vec<(NodeId, Status)> {
    let mut out: Vec<(NodeId, Status)> = now
        .pages()
        .iter()
        .map(|p| p.id)
        .filter(|p| !open.is_page(*p))
        .map(|p| match then.is_page(p) {
            true => (p, Status::Changed),
            false => (p, Status::Added),
        })
        .collect();
    out.extend(
        then.pages()
            .iter()
            .map(|p| p.id)
            .filter(|p| !open.is_page(*p) && !now.is_page(*p))
            .map(|p| (p, Status::Removed)),
    );
    out
}

/// Read the version pinned at `path`.
fn load(path: &Path) -> Result<Document, String> {
    std::fs::read(path)
        .map_err(|e| crate::private::io_error(&e))
        .and_then(|b| ondin_core::io::load(&b).map_err(|e| e.to_string()))
}

/// `id`'s ancestors, nearest first, in the document that holds it — the open one
/// first.
fn ancestors<'d>(
    id: NodeId,
    then: &'d Document,
    now: &'d Document,
) -> impl Iterator<Item = NodeId> + 'd {
    let doc = if now.contains(id) { now } else { then };
    std::iter::successors(doc.get(id).and_then(|n| n.parent()), move |p| {
        doc.get(*p).and_then(|n| n.parent())
    })
}

/// Whether `action` may run while a compare is open: what moves the view, saves
/// or leaves, never what edits. The palette and the view switches stay out with
/// the View and Snap menus: each reaches modes and overlays the compare does not
/// draw, and guarding every one of them is not worth what they add to a diff.
pub(crate) fn admits(action: &Action) -> bool {
    matches!(
        action,
        Action::Save
            | Action::Open
            | Action::NewDocument
            | Action::CloseDocument
            | Action::Tab(_)
            | Action::OpenSettings
            | Action::ExportAll
            | Action::ExportCode(_)
            | Action::ExportEditable(_)
            | Action::Cheatsheet
            | Action::ShowInLibrary
            | Action::VersionHistory
            | Action::CompareVersion
            | Action::ZoomIn
            | Action::ZoomOut
            | Action::ZoomReset
            | Action::ZoomFit
            | Action::StepPage(_)
            // Private mode masks names, so it stays reachable for a shared screen.
            | Action::ToggleView(ViewSwitch::Private)
            | Action::Escape
    )
}

/// A status's hue, the ink on its tag, and its sign.
pub(crate) fn look(status: Status) -> (egui::Color32, egui::Color32, &'static str) {
    match status {
        Status::Added => (ADDED, ADDED_INK, "+"),
        Status::Removed => (REMOVED, REMOVED_INK, "−"),
        Status::Changed => (CHANGED, CHANGED_INK, "~"),
    }
}

/// A change outside every page: added with no value before, removed with none after.
fn status_of(c: &Change) -> Status {
    match (&c.before, &c.after) {
        (None, Some(_)) => Status::Added,
        (Some(_), None) => Status::Removed,
        _ => Status::Changed,
    }
}

impl OndinApp {
    /// Open a compare of the document with the version `pick`, or of `pick` with
    /// `newer`, a later version, which then stands for Now. Says why when it opens
    /// none.
    pub(crate) fn compare_with(&mut self, pick: Pick, newer: Option<Pick>) {
        self.finish_text_first();
        let newer = match newer.map(|n| load(&n.path).map(|doc| (n, doc))).transpose() {
            Ok(n) => n.map(|(pick, doc)| Newer {
                pick,
                res: Resolved::rebuild(&doc),
                doc,
            }),
            Err(e) => return self.session.fail(format!("Compare failed: {e}")),
        };
        let then = match load(&pick.path) {
            Ok(doc) => doc,
            Err(e) => return self.session.fail(format!("Compare failed: {e}")),
        };
        let then_res = Resolved::rebuild(&then);
        let (now, now_res) = match &newer {
            Some(n) => (&n.doc, &n.res),
            None => (&self.session.doc, &self.session.resolved),
        };
        let (groups, rows, off_canvas) = rows(&then, now, &then_res, now_res);
        if rows.is_empty() && off_canvas.is_empty() {
            return self.session.info(match newer {
                Some(_) => "These versions are the same",
                None => "Nothing has changed since this version",
            });
        }
        let elsewhere = elsewhere(&then, now, &self.session.doc);
        // One open is left first, so the selection it took aside comes back to be taken.
        self.leave_compare();
        // Their overlays and fixes are the editing's, not the compare's.
        if self.find.open {
            self.close_find();
        }
        self.end_contrast();
        self.vision = None;
        self.session.clear_gesture_preview();
        let selection = (
            self.session.page,
            std::mem::take(&mut self.session.selection),
        );
        let stamp = crate::session::next_revision();
        self.session.compare = Some(Box::new(Compare {
            pick,
            then,
            then_res,
            newer,
            stamp,
            overrides: RenderOverrides::default(),
            ink: InkLog::default(),
            groups,
            rows,
            off_canvas,
            current: None,
            expanded: HashSet::new(),
            side: Side::Now,
            split: 0.5,
            split_from: None,
            overlay: true,
            reveal: None,
            selection,
            elsewhere,
            away: None,
        }));
    }

    pub(crate) fn leave_compare(&mut self) {
        self.come_back();
        if let Some(c) = self.session.compare.take()
            && c.selection.0 == self.session.page
        {
            self.session.selection = c.selection.1;
        }
    }

    /// The page the canvas shows: while comparing, possibly one the open document
    /// lacks ([`Compare::away`]); else the current page.
    pub(crate) fn compare_page(&self) -> NodeId {
        self.session
            .compare
            .as_ref()
            .and_then(|c| c.away)
            .map_or(self.session.page, |(page, _)| page)
    }

    /// Show `page`, which only the compared versions have: the open document's
    /// camera parked, the page fitted, and the side switched when the one showing
    /// has nothing there.
    pub(crate) fn go_away(&mut self, page: NodeId) {
        let camera = self.session.camera;
        let Some(c) = self.session.compare.as_mut() else {
            return;
        };
        let in_now = c.newer.as_ref().is_some_and(|n| n.doc.is_page(page));
        let in_then = c.then.is_page(page);
        if !(in_now || in_then) || c.away.is_some_and(|(p, _)| p == page) {
            return;
        }
        let left = c.away.map_or(camera, |(_, left)| left);
        c.away = Some((page, left));
        c.side = match c.side {
            Side::Now if !in_now => Side::Then,
            Side::Then if !in_then => Side::Now,
            side => side,
        };
        self.session.camera = crate::view::Camera::default();
        self.zoom_to(self.document_bounds());
    }

    /// Back from a page the open document lacks, to its own camera.
    pub(crate) fn come_back(&mut self) {
        if let Some((_, camera)) = self.session.compare.as_mut().and_then(|c| c.away.take()) {
            self.session.camera = camera;
        }
    }

    /// Draw the marks, or stop drawing them: the bar's *Overlay* and `O`.
    pub(crate) fn toggle_overlay(&mut self) {
        if let Some(c) = self.session.compare.as_mut() {
            c.overlay = !c.overlay;
        }
    }

    /// A row picked in the changes card: the overlay comes back to show it, then
    /// the row is shown as a step would show it.
    pub(crate) fn show_card_row(&mut self, i: usize) {
        if let Some(c) = self.session.compare.as_mut() {
            c.overlay = true;
        }
        self.show_change(i);
    }

    /// While comparing, take a plain `O` for the overlay before the keymap reads
    /// it as the Ellipse tool's letter, which a read-only compare would refuse
    /// with a toast. Under the bar's own conditions for its keys.
    pub(crate) fn compare_overlay_key(&mut self, ctx: &egui::Context) {
        if self.session.compare.is_none()
            || self.modal_is_up()
            || self.palette.is_some()
            || ctx.egui_wants_keyboard_input()
        {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::O)) {
            self.toggle_overlay();
        }
    }

    /// Make the row `by` steps from the current one current, round the ends.
    pub(crate) fn step_compare(&mut self, by: isize) {
        let Some(c) = self.session.compare.as_ref() else {
            return;
        };
        let n = c.rows.len();
        if n == 0 {
            return;
        }
        let at = match c.current {
            None if by > 0 => 0,
            None => n - 1,
            Some(i) => (i as isize + by).rem_euclid(n as isize) as usize,
        };
        self.show_change(at);
    }

    /// Make row `i` current and show it: its page, then the view panned to it by
    /// the least that will do, the zoom kept.
    fn show_change(&mut self, i: usize) {
        let Some(c) = self.session.compare.as_mut() else {
            return;
        };
        let Some(page) = c.rows.get(i).map(|r| r.page) else {
            return;
        };
        c.current = Some(i);
        c.reveal = Some(i);
        match page {
            Some(page) if self.session.doc.is_page(page) => self.go_to_page(page),
            Some(page) => self.go_away(page),
            None => {}
        }
    }

    /// Reveal a stepped-to row once its page is showing: after the canvas, so a
    /// first visit's framing is not drawn over it.
    fn reveal_change(&mut self) {
        let shown = self.compare_page();
        let Some(c) = self.session.compare.as_mut() else {
            return;
        };
        let Some(row) = c.reveal.and_then(|i| c.rows.get(i)) else {
            return;
        };
        if row.page.is_some_and(|p| p != shown) {
            return;
        }
        let bounds = match (row.shown_at(), row.moved().then_some(row.then).flatten()) {
            (Some(a), Some(b)) => Some(a.union(b)),
            (a, b) => a.or(b),
        };
        c.reveal = None;
        if let Some(b) = bounds.filter(|_| row.page.is_some()) {
            self.reveal(b);
        }
    }

    /// The compare's bar over the canvas's top, and its keys: Enter and
    /// Shift+Enter step, `\` toggles Then and Now. Drops the version's canvas
    /// once no compare is open.
    pub(crate) fn compare_bar(&mut self, ctx: &egui::Context) {
        if self.session.compare.is_none() {
            self.then_canvas = None;
            self.now_canvas = None;
            return;
        }
        self.reveal_change();
        let mut act = None;
        let free =
            !self.modal_is_up() && self.palette.is_none() && !ctx.egui_wants_keyboard_input();
        if free {
            ctx.input(|i| {
                if i.key_pressed(egui::Key::Enter) {
                    act = Some(Act::Step(if i.modifiers.shift { -1 } else { 1 }));
                } else if i.modifiers.is_none() && i.key_pressed(egui::Key::Backslash) {
                    act = Some(Act::Toggle);
                }
            });
        }

        // Where the contrast checker's and the vision bar's sit: below the top
        // ruler, centred on the canvas.
        let ruler = if self.rulers_on() {
            crate::rulers::THICKNESS
        } else {
            0.0
        };
        let anchor = self.canvas_rect.center_top() + egui::vec2(0.0, 12.0 + ruler);
        let private = crate::private::on();
        let Some(c) = self.session.compare.as_mut() else {
            return;
        };
        egui::Area::new(egui::Id::new("compare-bar"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                crate::contrast::bar_frame().show(ui, |ui| {
                    ui.set_height(crate::contrast::BAR_H - 2.0);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        bar(ui, c, private, &mut act);
                    });
                });
            });

        match act {
            // The bar expands nothing.
            None | Some(Act::Expand(_)) => {}
            Some(Act::Step(by)) => self.step_compare(by),
            Some(Act::Show(i)) => self.show_change(i),
            Some(Act::Toggle) => {
                if let Some(c) = self.session.compare.as_mut() {
                    c.side = match c.side {
                        Side::Now => Side::Then,
                        _ => Side::Now,
                    };
                }
            }
            Some(Act::Side(side)) => {
                if let Some(c) = self.session.compare.as_mut() {
                    c.side = side;
                }
            }
            Some(Act::Overlay) => self.toggle_overlay(),
            Some(Act::Done) => self.leave_compare(),
        }
    }

    /// The changes card, in the inspector's place while a compare is open.
    pub(crate) fn compare_card(&mut self, ctx: &egui::Context) {
        let column = Self::inspector_column(ctx.content_rect());
        let mut act = None;
        let Some(c) = self.session.compare.as_ref() else {
            return;
        };
        let doc = c.now_doc(&self.session.doc);
        egui::Area::new(egui::Id::new("compare-card"))
            .order(egui::Order::Middle)
            .fixed_pos(column.min)
            .show(ctx, |ui| {
                ui.set_width(column.width());
                egui::Frame::NONE
                    .fill(color::card())
                    .corner_radius(5)
                    .stroke(egui::Stroke::new(1.0, color::card_border()))
                    .inner_margin(egui::Margin {
                        left: 6,
                        right: 6,
                        top: 12,
                        bottom: 8,
                    })
                    .show(ui, |ui| {
                        ui.set_width(column.width() - 12.0);
                        ui.spacing_mut().item_spacing.y = ITEM_GAP;
                        card_head(ui, c);
                        egui::ScrollArea::vertical()
                            .max_height(column.height() - 60.0)
                            .auto_shrink([false, true])
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .show_viewport(ui, |ui, viewport| {
                                card_list(ui, viewport, c, doc, &mut act)
                            });
                    });
            });
        match act {
            Some(Act::Show(i)) => self.show_card_row(i),
            Some(Act::Expand(i)) => {
                if let Some(c) = self.session.compare.as_mut()
                    && !c.expanded.remove(&i)
                {
                    c.expanded.insert(i);
                }
            }
            _ => {}
        }
    }

    /// The canvas's input while comparing: any drag pans, the Swipe handle's
    /// drags it, and a click on a mark makes it current.
    pub(crate) fn compare_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let Some(c) = self.session.compare.as_ref() else {
            return;
        };
        let split_x = rect.left() + rect.width() * c.split;
        let swipe = c.side == Side::Swipe;
        let near = |p: egui::Pos2| swipe && (p.x - split_x).abs() <= HANDLE_REACH;
        let dragging = c.split_from.is_some();
        let on_handle = resp.hover_pos().is_some_and(near);
        // A drag a right-click cancelled stays spent until the button is let go:
        // neither the split nor the camera follows the hand meanwhile.
        let spent = self.gesture_cancelled;
        if resp.drag_started() && !spent {
            let origin = ui.input(|i| i.pointer.press_origin());
            let grab = origin.is_some_and(near) && resp.dragged_by(egui::PointerButton::Primary);
            if let Some(c) = self.session.compare.as_mut() {
                c.split_from = grab.then_some(c.split);
            }
        }
        if resp.dragged() && !spent {
            if self
                .session
                .compare
                .as_ref()
                .is_some_and(|c| c.split_from.is_some())
            {
                if let Some(p) = resp.interact_pointer_pos()
                    && let Some(c) = self.session.compare.as_mut()
                {
                    c.split = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                }
            } else {
                self.pan_by_drag(ui, resp);
            }
        }
        if resp.drag_stopped()
            && let Some(c) = self.session.compare.as_mut()
        {
            c.split_from = None;
        }
        let cursor = if dragging || on_handle {
            egui::CursorIcon::ResizeHorizontal
        } else if resp.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Default
        };
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(cursor);
        }
        // Marks the overlay hides cannot be clicked.
        if resp.clicked()
            && self.session.compare.as_ref().is_some_and(|c| c.overlay)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(i) = self.compare_mark_at(p, rect, ppp)
            && let Some(c) = self.session.compare.as_mut()
        {
            c.current = Some(i);
        }
    }

    /// A row's mark on screen: its box, outset; `None` off this page.
    fn mark_rect(&self, row: &Row, rect: egui::Rect, ppp: f32) -> Option<egui::Rect> {
        if row.is_page || row.page != Some(self.compare_page()) {
            return None;
        }
        let b = row.shown_at()?;
        let a = self.to_screen(ondin_core::kurbo::Point::new(b.x0, b.y0), rect, ppp);
        let z = self.to_screen(ondin_core::kurbo::Point::new(b.x1, b.y1), rect, ppp);
        Some(egui::Rect::from_two_pos(a, z).expand(MARK_OUTSET))
    }

    /// The row whose mark holds `p`, the smallest of several.
    fn compare_mark_at(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<usize> {
        let c = self.session.compare.as_ref()?;
        c.rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| Some((i, self.mark_rect(row, rect, ppp)?)))
            .filter(|(_, r)| r.contains(p))
            .min_by(|(_, a), (_, b)| a.area().total_cmp(&b.area()))
            .map(|(i, _)| i)
    }

    /// The compare over the drawn document: the version where Then or Swipe
    /// shows it, the newer version or an empty ground over Now where the open
    /// document is not what Now shows, the frame labels where it is, the marks, and
    /// Swipe's handle.
    pub(crate) fn paint_compare(
        &mut self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let Some(side) = self.session.compare.as_ref().map(|c| c.side) else {
            return;
        };
        let split_x = self
            .session
            .compare
            .as_ref()
            .map_or(rect.center().x, |c| rect.left() + rect.width() * c.split);
        let page = self.compare_page();
        let away = page != self.session.page;
        let two = self
            .session
            .compare
            .as_ref()
            .is_some_and(|c| c.newer.is_some());
        let left = egui::Rect::from_min_max(rect.min, egui::pos2(split_x, rect.max.y));
        let right = egui::Rect::from_min_max(egui::pos2(split_x, rect.min.y), rect.max);
        let (then_clip, now_clip) = match side {
            Side::Then => (Some(rect), None),
            Side::Now => (None, Some(rect)),
            Side::Swipe => (Some(left), Some(right)),
        };
        // Now over the open document's drawing only when it is not that.
        let now_clip = now_clip.filter(|_| two || away);
        if then_clip.is_some() && self.then_canvas.is_none() {
            self.then_canvas = self.canvas.another();
        }
        if two && now_clip.is_some() && self.now_canvas.is_none() {
            self.now_canvas = self.canvas.another();
        }
        let (camera, px) = (self.session.camera, self.canvas_px);
        let view = (&camera, px, page, rect);
        if let Some(c) = self.session.compare.as_ref() {
            if let Some(clip) = now_clip {
                let clip = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
                let doc = c.newer.as_ref().map(|n| (&n.doc, &n.res));
                paint_version(&clip, self.now_canvas.as_mut(), c, doc, view);
            }
            if let Some(clip) = then_clip {
                let clip = painter.with_clip_rect(clip.intersect(painter.clip_rect()));
                let doc = Some((&c.then, &c.then_res));
                paint_version(&clip, self.then_canvas.as_mut(), c, doc, view);
            }
        }
        // The labels are the open document's, of its current page.
        if !away && !two {
            self.draw_frame_labels(ui, painter, rect, ppp);
        }

        let Some(c) = self.session.compare.as_ref() else {
            return;
        };
        let marks: Vec<(usize, egui::Rect)> = c
            .rows
            .iter()
            .enumerate()
            .filter(|_| c.overlay)
            .filter_map(|(i, row)| Some((i, self.mark_rect(row, rect, ppp)?)))
            .collect();
        let canvas = painter.with_clip_rect(painter.clip_rect().intersect(rect));
        // The current mark last, over its neighbours.
        let mut order: Vec<&(usize, egui::Rect)> = marks.iter().collect();
        order.sort_by_key(|(i, _)| c.current == Some(*i));
        for (i, at) in order {
            let row = &c.rows[*i];
            let current = c.current == Some(*i);
            if row.status == Status::Changed && row.moved() {
                self.paint_ghost(&canvas, row, *at, rect, ppp);
            }
            let name = match row.status {
                Status::Removed => crate::private::layer(&c.then, row.id),
                _ => crate::private::layer(c.now_doc(&self.session.doc), row.id),
            };
            paint_mark(&canvas, row, &name, *at, current);
        }

        if side == Side::Swipe {
            paint_handle(&canvas, rect, split_x);
        }
    }

    /// Where a moved or resized layer was: a dashed box and a line to where it is.
    fn paint_ghost(
        &self,
        painter: &egui::Painter,
        row: &Row,
        at: egui::Rect,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let Some(b) = row.then else { return };
        let a = self.to_screen(ondin_core::kurbo::Point::new(b.x0, b.y0), rect, ppp);
        let z = self.to_screen(ondin_core::kurbo::Point::new(b.x1, b.y1), rect, ppp);
        let was = egui::Rect::from_two_pos(a, z).expand(MARK_OUTSET);
        let stroke = egui::Stroke::new(1.0, REMOVED.gamma_multiply(0.8));
        dashed_rect(painter, was, stroke);
        painter.line_segment([was.center(), at.center()], egui::Stroke::new(1.0, REMOVED));
    }
}

/// What a frame of the bar or the card asked for.
enum Act {
    Step(isize),
    Show(usize),
    /// Expand a row with folded layers, or fold it again.
    Expand(usize),
    Side(Side),
    Toggle,
    /// Draw the marks, or stop drawing them.
    Overlay,
    Done,
}

/// The bar's contents: what is compared, the three sides, the steps, close.
fn bar(ui: &mut egui::Ui, c: &Compare, private: bool, act: &mut Option<Act>) {
    ui.label(theme::icon_text(icon::GIT_DIFF, 15.0, color::accent_400()));
    let mut job = egui::text::LayoutJob::default();
    let font = egui::FontId::proportional(crate::ui::Scale::Toolbar.text_pt());
    let part = |job: &mut egui::text::LayoutJob, text: &str, ink: egui::Color32| {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: ink,
                ..Default::default()
            },
        );
    };
    // A version by its name, its time beside it; in private mode, by neither.
    let name = |job: &mut egui::text::LayoutJob, pick: &Pick, masked: &str| {
        if private {
            return part(job, masked, color::text());
        }
        part(job, &pick.title, color::text());
        if let Some(when) = &pick.when {
            part(job, &format!(" · {when}"), theme::text::dim());
        }
    };
    part(&mut job, "Comparing ", theme::text::muted());
    name(&mut job, &c.pick, "a version");
    match &c.newer {
        Some(n) => {
            part(&mut job, " with ", theme::text::muted());
            name(&mut job, &n.pick, "another");
        }
        None => part(&mut job, " with now", theme::text::muted()),
    }
    ui.label(job);

    const SIDES: [(Side, &str, &str); 3] = [
        (Side::Then, "Then", "The version"),
        (
            Side::Now,
            "Now",
            "The document as it is — \\ toggles the two",
        ),
        (
            Side::Swipe,
            "Swipe",
            "The version left of a handle, now right of it",
        ),
    ];
    let lit = SIDES.iter().position(|(s, ..)| *s == c.side).unwrap_or(1);
    ui.add_space(5.0);
    if let Some(i) = crate::ui::segmented_tipped(
        ui,
        130.0,
        crate::ui::TOOLBAR_CELL_H,
        SIDES.len(),
        lit,
        |_| true,
        |i| SIDES[i].2,
        |p, i, rect, on| crate::ui::segment_label(p, rect, SIDES[i].1, on),
    ) {
        *act = Some(Act::Side(SIDES[i].0));
    }
    let state = match c.overlay {
        true => crate::ui::FieldButton::On,
        false => crate::ui::FieldButton::Off,
    };
    // The marks are boxes round the changed layers, so the glyph is one. As tall as
    // the segmented track beside it.
    if crate::ui::field_button(ui, icon::BOUNDING_BOX, crate::ui::TOOLBAR_H, state)
        .on_hover_text("Overlay: the change marks on the canvas (O)")
        .clicked()
    {
        *act = Some(Act::Overlay);
    }
    ui.add_space(5.0);

    crate::contrast::separator(ui);

    // The contrast checker's count, position and steps.
    let n = c.rows.len();
    ui.label(
        egui::RichText::new(crate::ui::count(n, "change"))
            .size(crate::ui::Scale::Toolbar.text_pt())
            .color(color::text()),
    );
    // Room for the widest position, so stepping never moves the arrows.
    let at_w = ui
        .painter()
        .layout_no_wrap(
            format!("{n} / {n}"),
            egui::FontId::proportional(crate::ui::Scale::Toolbar.text_pt()),
            theme::text::dim(),
        )
        .size()
        .x;
    crate::contrast::cell(ui, at_w.ceil() + 2.0, |ui| {
        if let Some(i) = c.current {
            ui.label(
                egui::RichText::new(format!("{} / {n}", i + 1))
                    .size(crate::ui::Scale::Toolbar.text_pt())
                    .color(theme::text::dim()),
            );
        }
    });
    ui.spacing_mut().item_spacing.x = 2.0;
    if crate::ui::icon_button(ui, icon::CARET_LEFT, 24.0, 13.0, false, n > 0)
        .on_hover_text("Previous change (Shift+Enter)")
        .clicked()
    {
        *act = Some(Act::Step(-1));
    }
    if crate::ui::icon_button(ui, icon::CARET_RIGHT, 24.0, 13.0, false, n > 0)
        .on_hover_text("Next change (Enter)")
        .clicked()
    {
        *act = Some(Act::Step(1));
    }
    if crate::ui::icon_button(ui, icon::X, 24.0, 13.0, false, true)
        .on_hover_text("Leave the compare (Esc)")
        .clicked()
    {
        *act = Some(Act::Done);
    }
}

/// The card's heading: *Changes*, and how many layers on how many pages, and how
/// many more off the canvas.
fn card_head(ui: &mut egui::Ui, c: &Compare) {
    let layers: usize = c.groups.iter().map(|g| g.layers).sum();
    let mut sum = vec![
        crate::ui::count(layers, "layer"),
        crate::ui::count(c.groups.len(), "page"),
    ];
    if !c.off_canvas.is_empty() {
        sum.push(format!("{} more", c.off_canvas.len()));
    }
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 18.0), egui::Sense::hover());
    let p = ui.painter();
    crate::ui::paint_eyebrow(
        p,
        egui::pos2(rect.left() + ROW_PAD, rect.top() + 6.0),
        egui::Align2::LEFT_CENTER,
        "Changes",
    );
    p.text(
        egui::pos2(rect.right() - ROW_PAD, rect.top() + 6.0),
        egui::Align2::RIGHT_CENTER,
        sum.join(" · "),
        egui::FontId::proportional(crate::ui::CAPTION_PT),
        theme::text::dim(),
    );
}

/// One entry of the card's list, in order.
#[derive(Clone, Copy, Debug, PartialEq, Hash)]
enum Item {
    /// A page's heading, by its index in [`Compare::groups`], and its row count.
    Heading(usize, usize),
    Row(usize),
    /// The gap above *Not on the canvas*.
    Gap,
    OffHeading,
    Off(usize),
}

/// A page heading's height, and *Not on the canvas*'s.
const HEADING_H: f32 = 24.0;
/// An off-canvas row's height: its title and one line of values.
const OFF_ROW_H: f32 = 5.0 + 16.0 + 3.0 + 14.0 + 5.0;
/// The gap above *Not on the canvas*.
const OFF_GAP: f32 = 6.0;
/// The card list's gap between entries.
const ITEM_GAP: f32 = 2.0;

/// The card's entries with each one's top and height, from the list's top: the
/// rows grouped by page in the document's order, then what is not on the canvas.
fn card_items(c: &Compare, private: bool) -> Vec<(Item, f32, f32)> {
    let mut by_group: Vec<Vec<usize>> = vec![Vec::new(); c.groups.len()];
    for (i, row) in c.rows.iter().enumerate() {
        if let Some(g) = by_group.get_mut(row.group) {
            g.push(i);
        }
    }
    let mut out = Vec::new();
    let mut y = 0.0;
    let mut push = |item: Item, h: f32| {
        if item == Item::Gap {
            y += h;
            return;
        }
        out.push((item, y, h));
        y += h + ITEM_GAP;
    };
    for (g, rows) in by_group.iter().enumerate() {
        push(Item::Heading(g, rows.len()), HEADING_H);
        for &i in rows {
            let lines = row_line_count(&c.rows[i], private, c.expanded.contains(&i));
            push(Item::Row(i), row_h(lines));
        }
    }
    if !c.off_canvas.is_empty() {
        push(Item::Gap, OFF_GAP);
        push(Item::OffHeading, HEADING_H);
        for k in 0..c.off_canvas.len() {
            push(Item::Off(k), OFF_ROW_H);
        }
    }
    out
}

/// The card's list, only the entries in view (and one past each end, so `Tab`
/// finds the next row) drawn; it follows a row the keyboard focuses.
fn card_list(
    ui: &mut egui::Ui,
    viewport: egui::Rect,
    c: &Compare,
    doc: &Document,
    act: &mut Option<Act>,
) {
    let private = crate::private::on();
    let items = card_items(c, private);
    let (top, left, width) = (
        ui.max_rect().top(),
        ui.max_rect().left(),
        ui.available_width(),
    );
    let total = items.last().map_or(0.0, |(_, y, h)| y + h);
    ui.set_height(total);
    let first = items
        .partition_point(|(_, y, h)| y + h < viewport.min.y)
        .saturating_sub(1);
    let last = (items.partition_point(|(_, y, _)| *y <= viewport.max.y) + 1).min(items.len());
    for &(item, y, h) in &items[first..last] {
        let rect = egui::Rect::from_min_size(egui::pos2(left, top + y), egui::vec2(width, h));
        // By the entry, so a row keeps its focus as the list scrolls past others.
        let id = egui::Id::new("compare-card-item").with(item);
        let mut ui = ui.new_child(egui::UiBuilder::new().id(id).max_rect(rect));
        let ui = &mut ui;
        match item {
            Item::Heading(g, count) => {
                let group = &c.groups[g];
                let name = group_name(c, doc, g, private);
                heading(ui, icon::FILE, &name, count, group.removed);
            }
            Item::Row(i) => {
                let row = &c.rows[i];
                let name = match private {
                    false => row.name.clone(),
                    true if row.is_page => group_name(c, doc, row.group, private),
                    true => match row.status {
                        Status::Removed => crate::private::layer(&c.then, row.id).into_owned(),
                        _ => crate::private::layer(doc, row.id).into_owned(),
                    },
                };
                let folded: Vec<String> = row
                    .folded
                    .iter()
                    .map(|f| match (private, f.status) {
                        (false, _) => f.name.clone(),
                        (true, Status::Removed) => {
                            crate::private::layer(&c.then, f.id).into_owned()
                        }
                        (true, _) => crate::private::layer(doc, f.id).into_owned(),
                    })
                    .collect();
                let expanded = c.expanded.contains(&i);
                let (resp, toggled) = row_ui(
                    ui,
                    row,
                    &name,
                    &folded,
                    private,
                    c.current == Some(i),
                    expanded,
                );
                if toggled {
                    *act = Some(Act::Expand(i));
                } else if resp.clicked() {
                    *act = Some(Act::Show(i));
                }
            }
            Item::Gap => {}
            Item::OffHeading => heading(
                ui,
                icon::SLIDERS,
                "Not on the canvas",
                c.off_canvas.len(),
                false,
            ),
            Item::Off(k) => off_canvas_row(ui, &c.off_canvas[k], private),
        }
    }
    crate::ui::keep_focus_in_view(ui);
}

/// Group `g`'s page name as the card heads it: masked in private mode.
fn group_name(c: &Compare, doc: &Document, g: usize, private: bool) -> String {
    let group = &c.groups[g];
    match (private, group.page) {
        (true, Some(page)) if doc.is_page(page) => crate::private::page(doc, page).into_owned(),
        (true, Some(page)) => crate::private::page(c.page_doc(page), page).into_owned(),
        (true, None) => "Page".into(),
        (false, _) => group.name.clone(),
    }
}

/// A page's heading in the list: its glyph, its name, how many rows it has.
fn heading(ui: &mut egui::Ui, glyph: &str, name: &str, count: usize, dim: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADING_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let ink = match dim {
        true => theme::text::faint(),
        false => theme::text::muted(),
    };
    let cy = rect.center().y;
    p.text(
        egui::pos2(rect.left() + ROW_PAD, cy),
        egui::Align2::LEFT_CENTER,
        glyph,
        theme::icon_font(12.0),
        ink,
    );
    let room = rect.width() - ROW_PAD * 2.0 - 18.0 - 24.0;
    let galley = one_line(ui, &[(name, crate::ui::SMALL_PT, ink, false)], room);
    p.galley(
        egui::pos2(rect.left() + ROW_PAD + 18.0, cy - galley.size().y / 2.0),
        galley,
        ink,
    );
    p.text(
        egui::pos2(rect.right() - ROW_PAD, cy),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(crate::ui::CAPTION_PT),
        ink,
    );
}

/// `parts` laid out as one line cut with an ellipsis at `width`: each part's text,
/// size, ink and whether it is struck through.
pub(crate) fn one_line(
    ui: &egui::Ui,
    parts: &[(&str, f32, egui::Color32, bool)],
    width: f32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: width.max(1.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        },
        ..Default::default()
    };
    for (text, size, ink, struck) in parts {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(*size),
                color: *ink,
                strikethrough: match struck {
                    true => egui::Stroke::new(1.0, *ink),
                    false => egui::Stroke::NONE,
                },
                ..Default::default()
            },
        );
    }
    ui.fonts_mut(|f| f.layout_job(job))
}

/// A change's values as its line shows them: what, then before struck → after.
fn value_parts(c: &Change) -> Vec<(String, f32, egui::Color32, bool)> {
    let pt = crate::ui::SMALL_PT;
    let mut out = vec![(format!("{} ", c.what), pt, theme::text::muted(), false)];
    match (&c.before, &c.after) {
        (Some(b), Some(a)) => {
            out.push((b.clone(), pt, theme::text::faint(), true));
            out.push((" → ".into(), pt, theme::text::muted(), false));
            out.push((a.clone(), pt, color::text(), false));
        }
        (None, Some(a)) => out.push((a.clone(), pt, color::text(), false)),
        (Some(b), None) => out.push((b.clone(), pt, theme::text::faint(), true)),
        (None, None) => {}
    }
    out
}

/// The line under a row past its first `LINES_SHOWN`.
fn more_line(n: usize) -> Change {
    Change {
        what: format!("and {}", crate::ui::count(n, "more change")),
        before: None,
        after: None,
    }
}

/// One line under a row's title: a changed value, or a folded layer's name, at
/// an indent past the value lines'.
enum Line {
    Value(Change, f32),
    Layer(Status, String),
}

/// `changes` as lines at `indent`: one *n changes* in private mode, else every one
/// or, when `cut`, the first `LINES_SHOWN` and a line summing up the rest.
fn value_lines(changes: &[Change], indent: f32, private: bool, cut: bool) -> Vec<Line> {
    let mut out = Vec::new();
    if private {
        if !changes.is_empty() {
            let what = crate::ui::count(changes.len(), "change");
            out.push(Line::Value(
                Change {
                    what,
                    before: None,
                    after: None,
                },
                indent,
            ));
        }
        return out;
    }
    let shown = if cut { LINES_SHOWN } else { usize::MAX };
    out.extend(
        changes
            .iter()
            .take(shown)
            .map(|c| Line::Value(c.clone(), indent)),
    );
    if changes.len() > shown {
        out.push(Line::Value(more_line(changes.len() - shown), indent));
    }
    out
}

/// The lines under a row's title: folded, its folded layers' changes named under
/// it and cut; expanded, its own, then each folded layer and all of its changes.
fn row_lines(row: &Row, folded_names: &[String], private: bool, expanded: bool) -> Vec<Line> {
    if !expanded || row.folded.is_empty() {
        return value_lines(&row.all_lines(), 0.0, private, true);
    }
    let mut out = value_lines(&row.lines, 0.0, private, false);
    for (f, name) in row.folded.iter().zip(folded_names) {
        out.push(Line::Layer(f.status, name.clone()));
        out.extend(value_lines(&f.lines, FOLDED_INDENT, private, false));
    }
    out
}

/// A value line's height under a row's title.
const ROW_LINE_H: f32 = 14.0;

/// A row's height with `lines` value lines under its title.
fn row_h(lines: usize) -> f32 {
    5.0 + 16.0 + lines as f32 * (3.0 + ROW_LINE_H) + 5.0
}

/// How many lines [`value_lines`] gives for `n` changes.
fn value_line_count(n: usize, private: bool, cut: bool) -> usize {
    match (private, cut) {
        (true, _) => usize::from(n > 0),
        (false, true) => n.min(LINES_SHOWN) + usize::from(n > LINES_SHOWN),
        (false, false) => n,
    }
}

/// How many lines [`row_lines`] gives, without making them: the card measures
/// every row each frame and draws only those in view.
fn row_line_count(row: &Row, private: bool, expanded: bool) -> usize {
    if !expanded || row.folded.is_empty() {
        let folded: usize = (row.folded.iter())
            .map(|f| match f.status {
                Status::Changed => f.lines.len(),
                _ => 1,
            })
            .sum();
        return value_line_count(row.lines.len() + folded, private, true);
    }
    let folded: usize = (row.folded.iter())
        .map(|f| 1 + value_line_count(f.lines.len(), private, false))
        .sum();
    value_line_count(row.lines.len(), private, false) + folded
}

/// One row of the list: the status tag, the trail dim and the name, then the
/// changed values a line each. A row with folded layers has a caret: expanded, it
/// lists each of them apart (`folded_names`, in order) with all their values.
/// Private mode draws no trail and no values. Answers whether the caret was hit.
fn row_ui(
    ui: &mut egui::Ui,
    row: &Row,
    name: &str,
    folded_names: &[String],
    private: bool,
    current: bool,
    expanded: bool,
) -> (egui::Response, bool) {
    let lines = row_lines(row, folded_names, private, expanded);
    let line_h = ROW_LINE_H;
    let h = row_h(lines.len());
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::click());
    let p = ui.painter();
    crate::ui::list_row_ground(p, rect, current, resp.hovered());
    let (hue, ink, sign) = look(row.status);
    let tag =
        egui::Rect::from_min_size(rect.min + egui::vec2(ROW_PAD, 5.0), egui::vec2(16.0, 16.0));
    p.rect_filled(tag, 3.0, hue);
    p.text(
        tag.center(),
        egui::Align2::CENTER_CENTER,
        sign,
        egui::FontId::proportional(crate::ui::SMALL_PT),
        ink,
    );
    let mut title = name.to_string();
    if row.inside > 0 {
        title.push_str(&format!(" · {} inside", row.inside));
    }
    let trail = if private { "" } else { row.trail.as_str() };
    let x = tag.right() + 7.0;
    let mut end = rect.right() - ROW_PAD;
    let mut toggled = false;
    if !row.folded.is_empty() {
        let caret = egui::Rect::from_min_max(
            egui::pos2(end - 16.0, tag.top()),
            egui::pos2(end, tag.bottom()),
        );
        let over = resp.hover_pos().is_some_and(|p| caret.contains(p));
        toggled = resp.clicked()
            && resp
                .interact_pointer_pos()
                .is_some_and(|p| caret.contains(p));
        p.text(
            caret.center(),
            egui::Align2::CENTER_CENTER,
            match expanded {
                true => icon::CARET_DOWN,
                false => icon::CARET_RIGHT,
            },
            theme::icon_font(12.0),
            match over {
                true => color::text(),
                false => theme::text::dim(),
            },
        );
        end = caret.left() - 4.0;
    }
    let galley = one_line(
        ui,
        &[
            (trail, crate::ui::CONTROL_PT, theme::text::faint(), false),
            (&title, crate::ui::CONTROL_PT, color::text(), false),
        ],
        end - x,
    );
    p.galley(
        egui::pos2(x, tag.center().y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    let mut y = tag.bottom() + 3.0;
    let left = rect.left() + ROW_PAD + LINE_INDENT;
    for line in &lines {
        match line {
            Line::Value(c, indent) => {
                let parts = value_parts(c);
                let refs: Vec<(&str, f32, egui::Color32, bool)> = parts
                    .iter()
                    .map(|(t, s, i, k)| (t.as_str(), *s, *i, *k))
                    .collect();
                let x = left + indent;
                let galley = one_line(ui, &refs, rect.right() - ROW_PAD - x);
                p.galley(egui::pos2(x, y), galley, theme::text::muted());
            }
            Line::Layer(status, name) => {
                let (hue, ink, sign) = look(*status);
                let tag =
                    egui::Rect::from_min_size(egui::pos2(left, y + 1.0), egui::vec2(12.0, 12.0));
                p.rect_filled(tag, 2.0, hue);
                p.text(
                    tag.center(),
                    egui::Align2::CENTER_CENTER,
                    sign,
                    egui::FontId::proportional(crate::ui::CAPTION_PT),
                    ink,
                );
                let x = left + FOLDED_INDENT;
                let galley = one_line(
                    ui,
                    &[(name, crate::ui::SMALL_PT, color::text(), false)],
                    rect.right() - ROW_PAD - x,
                );
                p.galley(egui::pos2(x, y), galley, color::text());
            }
        }
        y += line_h + 3.0;
    }
    (resp, toggled)
}

/// A change no page holds: its status tag and what, the values under it.
fn off_canvas_row(ui: &mut egui::Ui, c: &Change, private: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), OFF_ROW_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let (hue, ink, sign) = look(status_of(c));
    let tag =
        egui::Rect::from_min_size(rect.min + egui::vec2(ROW_PAD, 5.0), egui::vec2(16.0, 16.0));
    p.rect_filled(tag, 3.0, hue);
    p.text(
        tag.center(),
        egui::Align2::CENTER_CENTER,
        sign,
        egui::FontId::proportional(crate::ui::SMALL_PT),
        ink,
    );
    let x = tag.right() + 7.0;
    let galley = one_line(
        ui,
        &[(&c.what, crate::ui::CONTROL_PT, color::text(), false)],
        rect.right() - ROW_PAD - x,
    );
    p.galley(
        egui::pos2(x, tag.center().y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    if private {
        return;
    }
    let values = Change {
        what: String::new(),
        ..c.clone()
    };
    let parts = value_parts(&values);
    let refs: Vec<(&str, f32, egui::Color32, bool)> = parts
        .iter()
        .skip(1)
        .map(|(t, s, i, k)| (t.as_str(), *s, *i, *k))
        .collect();
    let x = rect.left() + ROW_PAD + LINE_INDENT;
    let galley = one_line(ui, &refs, rect.right() - ROW_PAD - x);
    p.galley(
        egui::pos2(x, tag.bottom() + 3.0),
        galley,
        theme::text::muted(),
    );
}

/// A row's mark: its outline, solid or dashed, and a tag above its top-left
/// corner naming it, no wider than the mark (cut short with an ellipsis, so a
/// long name never runs over its neighbours); the current one ringed in white
/// and tinted.
fn paint_mark(painter: &egui::Painter, row: &Row, name: &str, at: egui::Rect, current: bool) {
    let (hue, ink, sign) = look(row.status);
    if current {
        painter.rect_filled(at, 3.0, hue.gamma_multiply(0.1));
        painter.rect_stroke(
            at,
            3.0,
            egui::Stroke::new(2.0, egui::Color32::WHITE),
            egui::StrokeKind::Middle,
        );
    } else if row.status == Status::Removed {
        dashed_rect(painter, at, egui::Stroke::new(1.5, hue));
    } else {
        painter.rect_stroke(
            at,
            3.0,
            egui::Stroke::new(1.5, hue),
            egui::StrokeKind::Middle,
        );
    }
    let mut label = format!("{sign} {name}");
    if row.layers > 1 {
        label.push_str(&format!(" · {}", row.layers));
    }
    // A floor keeps the sign and a few letters over a layer narrower than them.
    let max_w = (at.width() + 2.0).max(TAG_MIN_W) - 10.0;
    let mut job = egui::text::LayoutJob::simple_singleline(
        label,
        egui::FontId::proportional(crate::ui::CAPTION_PT),
        ink,
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_w,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let size = egui::vec2(galley.size().x + 10.0, 16.0);
    let tag = egui::Rect::from_min_size(egui::pos2(at.left() - 1.0, at.top() - 19.0), size);
    painter.rect_filled(tag, 3.0, hue);
    painter.galley(
        egui::pos2(tag.left() + 5.0, tag.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
}

/// A compared document's `page` drawn through `canvas` over `rect`, within
/// `painter`'s clip; where `doc` is `None` or lacks the page, the theme's canvas
/// ground, empty.
fn paint_version(
    painter: &egui::Painter,
    canvas: Option<&mut CanvasRenderer>,
    c: &Compare,
    doc: Option<(&Document, &Resolved)>,
    (camera, px, page, rect): (&crate::view::Camera, (u32, u32), NodeId, egui::Rect),
) {
    let Some((doc, res)) = doc.filter(|(d, _)| d.is_page(page)) else {
        let ground = crate::ui::color_to_egui(crate::session::theme_canvas());
        painter.rect_filled(painter.clip_rect(), 0.0, ground);
        return;
    };
    let ground = crate::session::page_ground(doc, page);
    let stamps = (c.stamp, c.stamp, c.stamp);
    let tex = canvas.and_then(|canvas| {
        canvas.render(
            doc,
            res,
            &c.overrides,
            stamps,
            &c.ink,
            camera,
            px,
            ground,
            page,
        )
    });
    if let Some(tex) = tex {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(tex, rect, uv, egui::Color32::WHITE);
    }
}

/// The outline of `r` in dashes.
fn dashed_rect(painter: &egui::Painter, r: egui::Rect, stroke: egui::Stroke) {
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(&pts, stroke, 5.0, 3.0));
}

/// Swipe's split: a line down the canvas, a grip on it, and which side is which.
fn paint_handle(painter: &egui::Painter, rect: egui::Rect, x: f32) {
    let accent = color::accent();
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(2.0, accent),
    );
    let grip = egui::Rect::from_center_size(egui::pos2(x, rect.center().y), egui::vec2(24.0, 36.0));
    painter.rect_filled(grip, 6.0, accent);
    painter.text(
        grip.center(),
        egui::Align2::CENTER_CENTER,
        icon::ARROWS_LEFT_RIGHT,
        theme::icon_font(14.0),
        egui::Color32::from_rgb(0x10, 0x13, 0x1b),
    );
    for (text, align, at) in [
        (
            "THEN",
            egui::Align2::LEFT_BOTTOM,
            rect.left_bottom() + egui::vec2(10.0, -10.0),
        ),
        (
            "NOW",
            egui::Align2::RIGHT_BOTTOM,
            rect.right_bottom() + egui::vec2(-10.0, -10.0),
        ),
    ] {
        let galley = painter.layout_no_wrap(
            text.into(),
            egui::FontId::proportional(crate::ui::CAPTION_PT),
            egui::Color32::WHITE,
        );
        let pill = align.anchor_size(at, galley.size() + egui::vec2(12.0, 6.0));
        painter.rect_filled(pill, 3.0, egui::Color32::from_black_alpha(153));
        painter.galley(
            pill.min + egui::vec2(6.0, 3.0),
            galley,
            egui::Color32::WHITE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{NodeKind, Operation, Transaction};

    fn boxed(id: NodeId, parent: NodeId, name: &str, at: (f64, f64), frame: bool) -> Operation {
        let (size, corner_radii) = (Size::new(40.0, 40.0), RoundedRectRadii::default());
        Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: match frame {
                true => NodeKind::Artboard {
                    size,
                    corner_radii,
                    corner_smoothing: 0.0,
                },
                false => NodeKind::Rect {
                    size,
                    corner_radii,
                    corner_smoothing: 0.0,
                },
            },
            transform: Some(Affine::translate(at)),
            name: Some(name.into()),
        }
    }

    /// A version with a frame `Card` holding `Price`, and `Old`, on the first page,
    /// and `Pay` on a second page far off; since then the card was renamed, the price
    /// faded, `Seat note` added, `Old` deleted and `Pay` moved.
    struct F {
        app: OndinApp,
        version: PathBuf,
        first: NodeId,
        second: NodeId,
        card: NodeId,
        old: NodeId,
        pay: NodeId,
    }

    fn fixture(ctx: &egui::Context, tag: &str) -> F {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let first = app.session.page;
        let [second, card, price, old, pay, seat] = [(); 6].map(|_| app.session.ids.mint());
        assert!(app.session.commit(Transaction(vec![
            Operation::AddPage {
                id: second,
                index: 1,
                name: "Checkout".into(),
                ground: None,
                draft: false,
                modes: Vec::new(),
            },
            boxed(card, first, "Card", (0.0, 0.0), true),
            boxed(price, card, "Price", (5.0, 5.0), false),
            boxed(old, first, "Old", (100.0, 0.0), false),
            boxed(pay, second, "Pay", (5000.0, 5000.0), false),
        ])));
        let version =
            std::env::temp_dir().join(format!("ondin-compare-{tag}-{}.ondin", std::process::id()));
        std::fs::write(&version, ondin_core::io::save(&app.session.doc).unwrap()).unwrap();
        assert!(app.session.commit(Transaction(vec![
            Operation::SetName {
                id: card,
                name: "Studio".into(),
            },
            Operation::SetOpacity {
                id: price,
                opacity: 0.5,
            },
            boxed(seat, first, "Seat note", (0.0, 200.0), false),
            Operation::DeleteNode { id: old },
            Operation::SetTransform {
                id: pay,
                transform: Affine::translate((5000.0, 5032.0)),
            },
        ])));
        F {
            app,
            version,
            first,
            second,
            card,
            old,
            pay,
        }
    }

    /// One frame of the whole app, with `events` delivered to it.
    fn whole_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            modifiers: events
                .iter()
                .find_map(|e| match e {
                    egui::Event::Key { modifiers, .. } => Some(*modifiers),
                    _ => None,
                })
                .unwrap_or_default(),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            eframe::App::ui(app, ui, &mut frame);
        })
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn pick(path: &Path, title: &str) -> Pick {
        Pick {
            path: path.to_path_buf(),
            title: title.into(),
            when: None,
        }
    }

    fn open(f: &mut F) -> &Compare {
        f.app.compare_with(pick(&f.version, "Before review"), None);
        f.app.session.compare.as_deref().expect("the compare opens")
    }

    /// A compare refuses the palette and every view switch but private mode, and
    /// keeps the zoom and Escape.
    #[test]
    fn a_compare_refuses_the_palette_and_the_view_switches() {
        assert!(!admits(&Action::Palette));
        assert!(!admits(&Action::ToggleView(ViewSwitch::Rulers)));
        assert!(!admits(&Action::ToggleView(ViewSwitch::SnapGrid)));
        assert!(!admits(&Action::Undo));
        assert!(admits(&Action::ToggleView(ViewSwitch::Private)));
        assert!(admits(&Action::ZoomFit));
        assert!(admits(&Action::Escape));
    }

    /// A changed layer under a changed frame folds into the frame's row, named
    /// under it and counted; an added, a removed and a moved layer each have their
    /// own row on their page, and each page counts its changed layers.
    ///
    /// flip: `rows` without its fold under a listed ancestor → fails at "one row for
    /// the card".
    #[test]
    fn the_topmost_changed_layer_is_marked_and_folds_the_rest() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "rows");
        let (first, second, card, old, pay) = (f.first, f.second, f.card, f.old, f.pay);
        let c = open(&mut f);
        let row = |id: NodeId| c.rows.iter().find(|r| r.id == id);
        let card_rows = c.rows.iter().filter(|r| r.group == 0 && !r.is_page).count();
        assert_eq!(
            card_rows, 3,
            "one row for the card, the note and the old box: {:#?}",
            c.rows
        );
        let card = row(card).expect("the card's row");
        assert_eq!(card.layers, 2, "it stands for the price too");
        assert!(
            card.all_lines()
                .iter()
                .any(|l| l.what.starts_with("Price · ")),
            "the price's change is named under it: {:?}",
            card.all_lines()
        );
        let old = row(old).expect("the removed box's row");
        assert_eq!(old.status, Status::Removed);
        assert_eq!(old.page, Some(first), "drawn on its page");
        assert!(old.then.is_some() && old.now.is_none(), "where it was");
        let pay = row(pay).expect("the moved box's row");
        assert!(pay.moved(), "moved");
        assert_eq!(pay.page, Some(second));
        assert_eq!(c.changed_on(first), 4, "card, price, note and box");
        assert_eq!(c.changed_on(second), 1);
        let _ = std::fs::remove_file(&f.version);
    }

    /// A layer that was there before and moved into a new frame keeps its own row
    /// with its changes, beside the frame's added row, and the page counts both.
    ///
    /// flip: `topmost` folding into an added row → fails at "the price's own row".
    #[test]
    fn a_layer_moved_into_a_new_frame_keeps_its_row() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "moved-in");
        let tray = f.app.session.ids.mint();
        let price = f.app.session.doc.get(f.card).unwrap().children()[0];
        assert!(f.app.session.commit(Transaction(vec![
            boxed(tray, f.first, "Tray", (0.0, 300.0), true),
            Operation::Reparent {
                id: price,
                new_parent: tray,
                index: 0,
            },
        ])));
        let first = f.first;
        let c = open(&mut f);
        let row = c
            .rows
            .iter()
            .find(|r| r.id == price)
            .expect("the price's own row");
        assert_eq!(row.status, Status::Changed);
        let whats: Vec<&str> = row.lines.iter().map(|l| l.what.as_str()).collect();
        assert!(
            whats.contains(&"parent") && whats.contains(&"opacity"),
            "{whats:?}"
        );
        let tray = c
            .rows
            .iter()
            .find(|r| r.id == tray)
            .expect("the tray's row");
        assert_eq!((tray.status, tray.inside), (Status::Added, 0));
        let rows = c.rows.iter().filter(|r| r.group == 0 && !r.is_page);
        assert_eq!(
            c.changed_on(first),
            rows.map(|r| r.layers).sum::<usize>(),
            "the badge counts what the rows show"
        );
        let _ = std::fs::remove_file(&f.version);
    }

    /// A row's height counts the lines it draws, folded or expanded, in private
    /// mode or not.
    #[test]
    fn a_rows_line_count_is_the_lines_it_draws() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "line-count");
        let c = open(&mut f);
        for row in &c.rows {
            let names = vec![String::new(); row.folded.len()];
            for (private, expanded) in [(false, false), (false, true), (true, false), (true, true)]
            {
                assert_eq!(
                    row_line_count(row, private, expanded),
                    row_lines(row, &names, private, expanded).len(),
                    "{} private {private} expanded {expanded}",
                    row.name
                );
            }
        }
        let _ = std::fs::remove_file(&f.version);
    }

    /// The changes card draws only the rows in view, and `Tab` down it keeps the
    /// focused row in view.
    ///
    /// flip: `card_list` without `keep_focus_in_view` → fails at "the focus stays
    /// on the card's rows", Tab running off the rows drawn; drawing every row →
    /// fails at "only the rows in view".
    #[test]
    fn the_card_draws_the_rows_in_view_and_follows_the_focus() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "long");
        let dots: Vec<Operation> = (0..60)
            .map(|k| {
                let id = f.app.session.ids.mint();
                boxed(
                    id,
                    f.first,
                    &format!("Dot {k}"),
                    (k as f64 * 50.0, 600.0),
                    false,
                )
            })
            .collect();
        assert!(f.app.session.commit(Transaction(dots)));
        open(&mut f);
        // `whole_frame`'s screen.
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1320.0, 820.0));
        let column = OndinApp::inspector_column(screen);
        // The card's titles, not the marks' tags on the canvas.
        let drawn = |out: &egui::FullOutput| {
            (out.shapes.iter())
                .filter(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        // Below the screen too: the ink a long list would lay out.
                        t.galley.text().starts_with("Dot ") && column.x_range().contains(t.pos.x)
                    }
                    _ => false,
                })
                .count()
        };
        let mut out = whole_frame(&ctx, &mut f.app, Vec::new());
        for _ in 0..3 {
            out = whole_frame(&ctx, &mut f.app, Vec::new());
        }
        let shown = drawn(&out);
        assert!(shown > 5 && shown < 40, "only the rows in view: {shown}");

        let card = egui::LayerId::new(egui::Order::Middle, egui::Id::new("compare-card"));
        let focused_row = |ctx: &egui::Context| {
            let id = ctx.memory(|m| m.focused())?;
            ctx.viewport(|v| v.prev_pass.widgets.get(id).copied())
                .filter(|w| w.layer_id == card)
        };
        let first = ctx.viewport(|v| {
            (v.prev_pass.widgets.get_layer(card))
                .find(|w| w.sense.is_focusable())
                .map(|w| w.id)
        });
        ctx.memory_mut(|m| m.request_focus(first.expect("a row on the card")));
        let tab = key(egui::Key::Tab, egui::Modifiers::NONE);
        for step in 0..40 {
            whole_frame(&ctx, &mut f.app, vec![tab.clone()]);
            whole_frame(&ctx, &mut f.app, Vec::new());
            whole_frame(&ctx, &mut f.app, Vec::new());
            let w = focused_row(&ctx).expect("the focus stays on the card's rows");
            assert!(
                screen.contains_rect(w.rect),
                "in view after {} tabs: {:?}",
                step + 1,
                w.rect
            );
        }
        let _ = std::fs::remove_file(&f.version);
    }

    /// A folded row names its folded layers' changes under it; expanded, it lists
    /// each folded layer apart with its own values, indented under its name.
    ///
    /// flip: `row_lines` ignoring `expanded` → fails at "the price listed apart".
    #[test]
    fn an_expanded_row_lists_its_folded_layers_apart() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "expand");
        let card = f.card;
        let c = open(&mut f);
        let row = c.rows.iter().find(|r| r.id == card).unwrap();
        assert_eq!(row.folded.len(), 1, "the price is folded in: {row:#?}");
        let names = vec!["Price".to_string()];
        let folded = row_lines(row, &names, false, false);
        assert!(
            folded.iter().all(|l| matches!(l, Line::Value(_, 0.0))),
            "folded, every line is a value"
        );
        assert!(
            folded
                .iter()
                .any(|l| matches!(l, Line::Value(c, _) if c.what.starts_with("Price · ")))
        );
        let open = row_lines(row, &names, false, true);
        let at = open
            .iter()
            .position(|l| matches!(l, Line::Layer(Status::Changed, n) if n == "Price"))
            .expect("the price listed apart");
        assert!(
            matches!(&open[at + 1], Line::Value(c, i) if *i == FOLDED_INDENT && !c.what.contains(" · ")),
            "its value under it, named by itself"
        );
        let _ = std::fs::remove_file(&f.version);
    }

    /// A page deleted since the version is listed apart; stepping to it shows it
    /// from the version, fitted, with the open document's page still current, and
    /// going back to that page gives its camera back.
    ///
    /// flip: `show_change` sending a page the open document lacks to `go_to_page`
    /// → fails at "the deleted page is shown".
    #[test]
    fn a_page_only_the_version_has_is_shown_from_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "away");
        let [archive, note] = [(); 2].map(|_| f.app.session.ids.mint());
        assert!(f.app.session.commit(Transaction(vec![
            Operation::AddPage {
                id: archive,
                index: 2,
                name: "Archive".into(),
                ground: None,
                draft: false,
                modes: Vec::new(),
            },
            boxed(note, archive, "Note", (-3000.0, 800.0), false),
        ])));
        std::fs::write(
            &f.version,
            ondin_core::io::save(&f.app.session.doc).unwrap(),
        )
        .unwrap();
        assert!(f.app.session.commit(Transaction(vec![
            Operation::DeleteNode { id: note },
            Operation::RemovePage { id: archive },
        ])));
        for _ in 0..2 {
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        let camera = f.app.session.camera;
        let c = open(&mut f);
        assert_eq!(c.elsewhere, vec![(archive, Status::Removed)]);
        f.app.step_compare(1);
        whole_frame(&ctx, &mut f.app, Vec::new());
        assert_eq!(f.app.compare_page(), archive, "the deleted page is shown");
        assert_eq!(
            f.app.session.page, f.first,
            "the open document's page stays"
        );
        let c = f.app.session.compare.as_deref().unwrap();
        assert_eq!(c.side, Side::Then, "from the version");
        let view = f.app.session.camera.viewport(f.app.canvas_px).view;
        assert!(
            view.contains(ondin_core::kurbo::Point::new(-2980.0, 820.0)),
            "fitted: {view:?}"
        );
        f.app.go_to_page(f.first);
        assert_eq!(f.app.compare_page(), f.first);
        assert_eq!(
            f.app.session.camera.center, camera.center,
            "its camera back"
        );
        let _ = std::fs::remove_file(&f.version);
    }

    /// Shift+clicking two versions compares the older with the newer, whatever the
    /// order: Now is the newer version, not the open document, which has since lost
    /// a layer and a page the newer one has.
    ///
    /// flip: `compare_pick` answering the two the other way round → fails at the
    /// newer's title.
    #[test]
    fn two_versions_compare_the_older_with_the_newer() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "two");
        let drafts = f.app.session.ids.mint();
        assert!(f.app.session.commit(Transaction(vec![Operation::AddPage {
            id: drafts,
            index: 2,
            name: "Drafts".into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
        }])));
        let doc = &f.app.session.doc;
        let seat = *doc
            .get(f.first)
            .unwrap()
            .children()
            .iter()
            .find(|id| doc.get(**id).unwrap().name() == "Seat note")
            .unwrap();
        let newer = f.version.with_extension("newer.ondin");
        std::fs::write(&newer, ondin_core::io::save(&f.app.session.doc).unwrap()).unwrap();
        assert!(f.app.session.commit(Transaction(vec![
            Operation::DeleteNode { id: seat },
            Operation::RemovePage { id: drafts },
        ])));
        // Newest first, as the card lists them.
        let picks = [pick(&newer, "After"), pick(&f.version, "Before")];
        assert!(
            f.app.compare_pick(&picks, 1, true).is_none(),
            "the first picks"
        );
        assert_eq!(f.app.versions_pick.as_deref(), Some(f.version.as_path()));
        let (then, now) = f
            .app
            .compare_pick(&picks, 0, true)
            .expect("the second compares");
        assert!(f.app.versions_pick.is_none());
        f.app.compare_with(then, now);
        let c = f.app.session.compare.as_deref().expect("the compare opens");
        assert_eq!(
            c.newer.as_ref().map(|n| n.pick.title.as_str()),
            Some("After"),
            "the newer's title"
        );
        let row = c
            .rows
            .iter()
            .find(|r| r.id == seat)
            .expect("the note's row");
        assert_eq!(row.status, Status::Added, "added in the newer");
        assert!(row.now.is_some(), "its box from the newer version");
        assert_eq!(c.elsewhere, vec![(drafts, Status::Added)]);
        for side in [Side::Then, Side::Swipe, Side::Now] {
            f.app.session.compare.as_mut().unwrap().side = side;
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        let _ = std::fs::remove_file(&f.version);
        let _ = std::fs::remove_file(&newer);
    }

    /// Nothing changed since the version: no compare opens.
    #[test]
    fn an_unchanged_document_opens_no_compare() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "same");
        std::fs::write(
            &f.version,
            ondin_core::io::save(&f.app.session.doc).unwrap(),
        )
        .unwrap();
        f.app.compare_with(pick(&f.version, "Now"), None);
        assert!(f.app.session.compare.is_none());
        let _ = std::fs::remove_file(&f.version);
    }

    /// While comparing, an edit, an undo and a chord that edits change nothing;
    /// Escape leaves, and editing works again.
    ///
    /// flip: `commit_inner` without its compare refusal → fails at "the commit is
    /// refused".
    #[test]
    fn a_compare_refuses_every_edit_and_escape_leaves_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "readonly");
        open(&mut f);
        let rename = |app: &mut OndinApp| {
            app.session.commit(Transaction(vec![Operation::SetName {
                id: f.card,
                name: "Edited".into(),
            }]))
        };
        let revision = f.app.session.revision();
        rename(&mut f.app);
        assert_eq!(f.app.session.revision(), revision, "the commit is refused");
        assert!(!f.app.session.undo(), "and the undo");
        f.app.dispatch(&ctx, Action::SelectAll);
        assert!(f.app.session.selection.ids().is_empty(), "and a chord");

        for _ in 0..2 {
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        whole_frame(
            &ctx,
            &mut f.app,
            vec![key(egui::Key::Escape, egui::Modifiers::NONE)],
        );
        assert!(f.app.session.compare.is_none(), "Escape leaves");
        rename(&mut f.app);
        assert_ne!(
            f.app.session.revision(),
            revision,
            "and editing works again"
        );
        let _ = std::fs::remove_file(&f.version);
    }

    /// Shift+Enter from no current change steps round to the last, on the other
    /// page, whose view was left elsewhere: the page switches and the view comes to
    /// the change, every side drawn on the way without a device.
    ///
    /// flip: `reveal_change` returning before its `reveal` → fails at "in view".
    #[test]
    fn stepping_switches_page_and_reveals_the_change() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "step");
        for _ in 0..2 {
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        f.app.go_to_page(f.second);
        for _ in 0..2 {
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        f.app.session.camera.center = ondin_core::kurbo::Point::new(-9000.0, -9000.0);
        f.app.session.camera.set_zoom(1.0);
        f.app.go_to_page(f.first);
        whole_frame(&ctx, &mut f.app, Vec::new());
        open(&mut f);
        for side in [Side::Then, Side::Swipe, Side::Now] {
            f.app.session.compare.as_mut().unwrap().side = side;
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        whole_frame(
            &ctx,
            &mut f.app,
            vec![key(egui::Key::Enter, egui::Modifiers::SHIFT)],
        );
        whole_frame(&ctx, &mut f.app, Vec::new());
        let c = f.app.session.compare.as_deref().expect("still comparing");
        let current = c.current.map(|i| c.rows[i].id);
        assert_eq!(current, Some(f.pay), "the last change is current");
        assert_eq!(f.app.session.page, f.second, "on its page");
        let at = c.rows.iter().find(|r| r.id == f.pay).unwrap().now.unwrap();
        let view = f.app.session.camera.viewport(f.app.canvas_px).view;
        assert!(
            view.contains(at.center()),
            "in view: {view:?} around {at:?}"
        );
        let _ = std::fs::remove_file(&f.version);
    }

    /// `O` turns the overlay off without choosing the Ellipse tool or saying the
    /// compare is read-only, stepping leaves it off, and a row picked in the card
    /// turns it back on.
    ///
    /// flip: `compare_overlay_key` reading the key without consuming it → fails at
    /// "no read-only toast".
    #[test]
    fn o_hides_the_overlay_and_a_card_row_brings_it_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "overlay");
        for _ in 0..2 {
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        open(&mut f);
        assert!(
            f.app.session.compare.as_ref().unwrap().overlay,
            "on at first"
        );
        let tool = f.app.tool;
        whole_frame(
            &ctx,
            &mut f.app,
            vec![key(egui::Key::O, Default::default())],
        );
        let c = f.app.session.compare.as_deref().expect("still comparing");
        assert!(!c.overlay, "O hides it");
        assert_eq!(f.app.tool, tool, "the tool is not chosen");
        assert_ne!(f.app.session.status().text, READ_ONLY, "no read-only toast");
        whole_frame(
            &ctx,
            &mut f.app,
            vec![key(egui::Key::Enter, Default::default())],
        );
        let c = f.app.session.compare.as_deref().unwrap();
        assert!(c.current.is_some(), "fixture: the step landed");
        assert!(!c.overlay, "a step leaves it off");
        f.app.show_card_row(0);
        assert!(
            f.app.session.compare.as_ref().unwrap().overlay,
            "a card row brings it back"
        );
        let _ = std::fs::remove_file(&f.version);
    }

    /// A right-click during the Swipe handle's drag puts the split back where the
    /// drag began, not at the centre, and the hand still held moves nothing after.
    ///
    /// flip: `cancel_gesture` ignoring `split_from` → fails at "back where it began".
    #[test]
    fn a_right_click_puts_the_swipe_handle_back() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx, "swipe-cancel");
        for _ in 0..2 {
            whole_frame(&ctx, &mut f.app, Vec::new());
        }
        open(&mut f);
        let c = f.app.session.compare.as_mut().unwrap();
        c.side = Side::Swipe;
        c.split = 0.3;
        whole_frame(&ctx, &mut f.app, Vec::new());
        let rect = f.app.canvas_rect;
        let at = |x: f32| egui::pos2(rect.left() + rect.width() * x, rect.center().y);
        let button = |pos, button, pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        };
        let primary = egui::PointerButton::Primary;
        whole_frame(&ctx, &mut f.app, vec![egui::Event::PointerMoved(at(0.3))]);
        whole_frame(&ctx, &mut f.app, vec![button(at(0.3), primary, true)]);
        for x in [0.4, 0.5, 0.6] {
            whole_frame(&ctx, &mut f.app, vec![egui::Event::PointerMoved(at(x))]);
        }
        let split = f.app.session.compare.as_ref().unwrap().split;
        assert!(
            (split - 0.6).abs() < 0.01,
            "fixture: the drag moved it: {split}"
        );
        let secondary = egui::PointerButton::Secondary;
        whole_frame(&ctx, &mut f.app, vec![button(at(0.6), secondary, true)]);
        whole_frame(&ctx, &mut f.app, vec![button(at(0.6), secondary, false)]);
        let split = f.app.session.compare.as_ref().unwrap().split;
        assert!((split - 0.3).abs() < 1e-6, "back where it began: {split}");
        for x in [0.7, 0.8] {
            whole_frame(&ctx, &mut f.app, vec![egui::Event::PointerMoved(at(x))]);
        }
        whole_frame(&ctx, &mut f.app, vec![button(at(0.8), primary, false)]);
        let c = f.app.session.compare.as_ref().unwrap();
        assert!((c.split - 0.3).abs() < 1e-6, "the held hand moves nothing");
        assert_eq!(c.split_from, None, "and the drag is over");
        let _ = std::fs::remove_file(&f.version);
    }
}
