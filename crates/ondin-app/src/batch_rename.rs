//! Batch rename (`docs/design/app-canvas-chrome.md`, *Batch rename*): the box
//! that renames many layers at once — the selection, the page or every page.
//!
//! The text rule is `ondin_core::rename`'s; this module picks the layers, orders
//! and numbers them ([`plan_into`]), draws the box with its preview, and commits
//! every new name as one undo step. Nothing changes on the canvas until then.
//!
//! The document cannot change while the box is up, so what the plan reads of it
//! is taken once, at open ([`Snapshot`]), and a worker plans from that copy each
//! time a setting changes, sending the rows back in parts as it goes.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use ondin_core::kurbo;
use ondin_core::rename::{Find, Numbering, Piece, Rule, Span, Template};
use ondin_core::{Document, NodeId, Operation, Resolved, Transaction};
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// The box's width, and the gap between the top of the canvas and the box: the
/// palette's place.
const W: f32 = 520.0;
const TOP_GAP: f32 = 56.0;
/// The label column, and the gap after it.
const LABEL_W: f32 = 72.0;
const GAP: f32 = 10.0;
const PAD: f32 = 14.0;
const FIELD_H: f32 = 30.0;
/// The numbering controls, as tall as the Settings modal's fields.
const NUM_H: f32 = 28.0;
const ROW_H: f32 = 26.0;
/// The preview scrolls past this many rows.
const ROWS_SHOWN: f32 = 8.0;
/// The layers the worker plans between two parts it sends.
const PART: usize = 500;

/// What the box renames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    Selection,
    Page,
    Document,
}

/// Which layer of the run is first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Order {
    /// The layers tree, top to bottom.
    Layers,
    Reversed,
    /// Rows, top to bottom, each read left to right.
    LeftToRight,
    /// Columns, left to right, each read top to bottom.
    TopToBottom,
}

impl Order {
    const ALL: [Order; 4] = [
        Order::Layers,
        Order::Reversed,
        Order::LeftToRight,
        Order::TopToBottom,
    ];

    fn label(self) -> &'static str {
        match self {
            Order::Layers => "Layers order",
            Order::Reversed => "Layers order, reversed",
            Order::LeftToRight => "Left to right",
            Order::TopToBottom => "Top to bottom",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Order::Layers => icon::LIST_NUMBERS,
            Order::Reversed => icon::ARROWS_DOWN_UP,
            Order::LeftToRight => icon::ARROW_RIGHT,
            Order::TopToBottom => icon::ARROW_DOWN,
        }
    }

    /// How prefs spell it.
    fn key(self) -> &'static str {
        match self {
            Order::Layers => "layers",
            Order::Reversed => "reversed",
            Order::LeftToRight => "left-to-right",
            Order::TopToBottom => "top-to-bottom",
        }
    }

    fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|o| o.key() == key)
            .unwrap_or(Order::Layers)
    }
}

/// One layer as the plan reads it, taken when the box opens.
#[derive(Clone, Debug)]
struct Layer {
    id: NodeId,
    name: String,
    /// A variant in a set, whose name is made from its property values.
    variant: bool,
    /// Inside an instance, and not its root.
    in_instance: bool,
    /// Selected when the box opened.
    selected: bool,
    /// Under a layer that was.
    under_selected: bool,
    bounds: Option<kurbo::Rect>,
    glyph: &'static str,
    tint: egui::Color32,
}

/// One page's layers, in the layers tree's order.
#[derive(Debug)]
struct SnapPage {
    id: NodeId,
    name: String,
    layers: Vec<Layer>,
}

/// What the plan reads of the document, every page in the Pages list's order.
#[derive(Debug)]
pub(crate) struct Snapshot {
    pages: Vec<SnapPage>,
}

impl Snapshot {
    /// Read every page of `doc`, marking `selection` and the layers under it.
    pub(crate) fn take(doc: &Document, res: &Resolved, selection: &[NodeId]) -> Self {
        let chosen: HashSet<NodeId> = selection.iter().copied().collect();
        let pages = doc
            .page_ids()
            .map(|page| {
                let mut layers = Vec::new();
                // `(id, under a selected layer)`, pushed in child order so the last
                // child, the tree's top row, pops first.
                let mut stack = vec![(page, false)];
                while let Some((id, under)) = stack.pop() {
                    let Some(node) = doc.get(id) else { continue };
                    let selected = chosen.contains(&id);
                    if id != page {
                        let (glyph, tint) = crate::panels::layers::node_icon(node.kind());
                        layers.push(Layer {
                            id,
                            name: node.name().to_string(),
                            variant: !node.variant().is_empty(),
                            in_instance: node.parent().is_some_and(|p| {
                                ondin_core::component::instance_root(doc, p).is_some()
                            }),
                            selected,
                            under_selected: under,
                            bounds: res.world_bounds(id),
                            glyph,
                            tint,
                        });
                    }
                    let under = under || selected;
                    stack.extend(node.children().iter().map(|c| (*c, under)));
                }
                SnapPage {
                    id: page,
                    name: doc
                        .get(page)
                        .map_or_else(String::new, |n| n.name().to_string()),
                    layers,
                }
            })
            .collect();
        Self { pages }
    }
}

/// Everything the plan depends on besides the snapshot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    pub(crate) scope: Scope,
    /// The current page, which *This page* means.
    page: NodeId,
    pub(crate) find: String,
    /// *Rename to*, with its tokens spelled `[Name]`, `[Number]`, `[Group k]`.
    pub(crate) template: String,
    pub(crate) match_case: bool,
    pub(crate) pattern: bool,
    pub(crate) keep_case: bool,
    pub(crate) numbering: Numbering,
    pub(crate) order: Order,
    /// *Include layers inside*, at Selection.
    pub(crate) inside: bool,
    /// *Include layers inside instances*, at page and document scope.
    pub(crate) inside_instances: bool,
}

impl Settings {
    pub(crate) fn set_scope(&mut self, scope: Scope) {
        self.scope = scope;
        // Away from the selection, the case of what matches is out of view.
        self.keep_case = scope != Scope::Selection;
    }
}

/// Where the template field's caret goes on its next frame.
#[derive(Clone, Copy, Debug)]
enum Caret {
    /// Everything selected, so typing replaces it: how the box opens.
    All,
    At(usize),
}

/// The open box.
pub(crate) struct BatchRename {
    pub(crate) s: Settings,
    /// How many layers were selected at open; zero leaves *Selection* off.
    selected: usize,
    matcher: Matcher,
    /// The settings last handed to the worker.
    asked: Option<Settings>,
    /// What the preview draws: the newest plan, as far as its parts have come.
    plan: Plan,
    /// Whether `plan` has had a part for the newest settings yet; until then it
    /// still shows the previous plan.
    fresh: bool,
    /// Whether `plan` is whole for the newest settings.
    done: bool,
    /// The template's caret, in characters, while the field had it: where an
    /// insert button puts its token.
    caret: Option<usize>,
    focus: Option<Caret>,
}

/// One row of the preview.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Row {
    /// All pages: the heading over a page's rows.
    Page {
        name: String,
        count: usize,
    },
    Layer(LayerRow),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayerRow {
    pub(crate) id: NodeId,
    glyph: &'static str,
    tint: egui::Color32,
    pub(crate) old: String,
    /// The byte ranges of `old` Find matched.
    pub(crate) matched: Vec<Range<usize>>,
    pub(crate) new: New,
}

/// What a listed layer's name becomes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum New {
    /// Renamed: the new name's runs, new ones drawn in the accent.
    Name(Vec<Span>),
    /// Find matches nothing, or the rule gives back the same name.
    Unchanged,
    /// The new name would be empty, so the layer keeps its own.
    Empty,
    /// A variant in a set, whose name is made from its property values.
    Variant,
}

/// What the box would do with its settings — or one part of it, as the worker
/// sends it, which [`Plan::absorb`] adds to the whole.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub(crate) rows: Vec<Row>,
    /// The layers searched, past those left out inside instances.
    pub(crate) searched: usize,
    /// The layer rows listed.
    pub(crate) listed: usize,
    /// The page headings: all pages' pages with a listed row.
    pub(crate) pages: usize,
    /// Each layer that changes, and its new name, in the run's order.
    pub(crate) changes: Vec<(NodeId, String)>,
    /// Matches inside instances the box leaves out.
    pub(crate) skipped: usize,
    /// The pattern's groups, which the template can insert.
    pub(crate) groups: usize,
    /// Why Find renames nothing: a pattern that does not parse.
    pub(crate) error: Option<String>,
    /// The row of the newest page heading, which the rows after it count into.
    head: Option<usize>,
}

impl Plan {
    /// Add `part`'s rows and counts to this plan.
    pub(crate) fn absorb(&mut self, part: Plan) {
        for row in part.rows {
            match &row {
                Row::Page { .. } => {
                    self.pages += 1;
                    self.head = Some(self.rows.len());
                }
                Row::Layer(_) => {
                    self.listed += 1;
                    if let Some(Row::Page { count, .. }) =
                        self.head.and_then(|h| self.rows.get_mut(h))
                    {
                        *count += 1;
                    }
                }
            }
            self.rows.push(row);
        }
        self.changes.extend(part.changes);
        self.searched += part.searched;
        self.skipped += part.skipped;
        self.groups = self.groups.max(part.groups);
        if part.error.is_some() {
            self.error = part.error;
        }
    }

    /// Why Rename has nothing to do, when it has not.
    fn why_not(&self, find: &str) -> Option<String> {
        if let Some(e) = &self.error {
            return Some(e.clone());
        }
        if !self.changes.is_empty() {
            return None;
        }
        let matched: Vec<&New> = self
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Layer(l) if !l.matched.is_empty() => Some(&l.new),
                _ => None,
            })
            .collect();
        Some(
            if matched.is_empty() {
                if find.is_empty() {
                    "No layers to rename"
                } else {
                    "No layer matches Find"
                }
            } else if matched.iter().all(|n| **n == New::Empty) {
                "Every new name would be empty"
            } else if matched.iter().all(|n| **n == New::Variant) {
                "Variants are named by their properties"
            } else {
                "Nothing would change"
            }
            .to_string(),
        )
    }
}

/// `ids` in reading order: rows (or columns, `!rows`) gathered by bands — a layer
/// joins the first band its centre falls in — then each band read along. Layers
/// with no bounds keep their order at the end.
fn by_bands<'a>(layers: Vec<&'a Layer>, rows: bool) -> Vec<&'a Layer> {
    /// A row or column: the span of the layer that started it, and its layers
    /// with where each starts along it.
    struct Band<'a> {
        lo: f64,
        hi: f64,
        members: Vec<(&'a Layer, f64)>,
    }
    let (mut boxed, mut rest) = (Vec::new(), Vec::new());
    for l in layers {
        match l.bounds {
            Some(r) if rows => boxed.push((l, (r.y0, r.y1), r.x0)),
            Some(r) => boxed.push((l, (r.x0, r.x1), r.y0)),
            None => rest.push(l),
        }
    }
    boxed.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
    let mut bands: Vec<Band<'a>> = Vec::new();
    for (l, (lo, hi), along) in boxed {
        let centre = (lo + hi) / 2.0;
        match bands.iter_mut().find(|b| b.lo <= centre && centre <= b.hi) {
            Some(band) => band.members.push((l, along)),
            None => bands.push(Band {
                lo,
                hi,
                members: vec![(l, along)],
            }),
        }
    }
    let mut out = Vec::new();
    for mut band in bands {
        band.members.sort_by(|a, b| a.1.total_cmp(&b.1));
        out.extend(band.members.into_iter().map(|m| m.0));
    }
    out.extend(rest);
    out
}

/// Plan the rename `s` describes over `snap`: the layers in scope, each page's in
/// the run's order and the pages in the Pages list's, numbered as they change.
/// The plan goes to `emit` in parts, every [`PART`] layers; `emit` answering false
/// stops it.
pub(crate) fn plan_into(snap: &Snapshot, s: &Settings, emit: &mut dyn FnMut(Plan) -> bool) {
    let mut part = Plan::default();
    let rule = match Find::new(&s.find, s.match_case, s.pattern) {
        Ok(find) => {
            part.groups = find.groups();
            Some(Rule {
                find,
                template: Template::parse(&s.template),
                keep_case: s.keep_case,
            })
        }
        Err(e) => {
            part.error = Some(e);
            None
        }
    };
    let matches = |name: &str| rule.as_ref().is_some_and(|r| r.apply(name, "").is_some());

    // Only layers that change take a number, so a skipped one leaves no gap.
    let mut taken = 0usize;
    let mut since = 0usize;
    let every = s.scope == Scope::Selection;
    for page in &snap.pages {
        if s.scope == Scope::Page && page.id != s.page {
            continue;
        }
        let mut run = Vec::new();
        for l in &page.layers {
            match s.scope {
                Scope::Selection if l.selected || (s.inside && l.under_selected) => run.push(l),
                Scope::Selection => {}
                _ if !s.inside_instances && l.in_instance => {
                    if matches(&l.name) {
                        part.skipped += 1;
                    }
                }
                _ => run.push(l),
            }
        }
        let run = match s.order {
            Order::Layers => run,
            Order::Reversed => run.into_iter().rev().collect(),
            Order::LeftToRight => by_bands(run, true),
            Order::TopToBottom => by_bands(run, false),
        };
        let mut headed = false;
        for l in run {
            part.searched += 1;
            let applied = rule
                .as_ref()
                .and_then(|r| r.apply(&l.name, &s.numbering.nth(taken)));
            let (matched, new) = match applied {
                None => (Vec::new(), New::Unchanged),
                Some(r) if l.variant => (r.matched, New::Variant),
                Some(r) => match ondin_core::naming::usable(&r.text()) {
                    None => (r.matched, New::Empty),
                    Some(name) if name == l.name => (r.matched, New::Unchanged),
                    Some(name) => {
                        taken += 1;
                        part.changes.push((l.id, name));
                        (r.matched, New::Name(r.spans))
                    }
                },
            };
            if every || !matched.is_empty() {
                if s.scope == Scope::Document && !headed {
                    headed = true;
                    part.rows.push(Row::Page {
                        name: page.name.clone(),
                        count: 0,
                    });
                }
                part.rows.push(Row::Layer(LayerRow {
                    id: l.id,
                    glyph: l.glyph,
                    tint: l.tint,
                    old: l.name.clone(),
                    matched,
                    new,
                }));
            }
            since += 1;
            if since == PART {
                since = 0;
                if !emit(std::mem::take(&mut part)) {
                    return;
                }
            }
        }
    }
    emit(part);
}

/// The whole plan at once.
pub(crate) fn plan(snap: &Snapshot, s: &Settings) -> Plan {
    let mut whole = Plan::default();
    plan_into(snap, s, &mut |part| {
        whole.absorb(part);
        true
    });
    whole
}

/// What the worker sends: a part of the plan for one request, or that the
/// request's plan is whole.
enum Sent {
    Part(u64, Plan),
    Done(u64),
}

/// The thread that plans while the box is open. It takes the newest request,
/// skipping any it was sent while busy, and abandons a plan as soon as a newer
/// request is made. It ends when the box drops its sender.
struct Matcher {
    tx: Sender<(u64, Settings)>,
    rx: Receiver<Sent>,
    /// The newest request, which the thread reads between parts.
    latest: Arc<AtomicU64>,
    /// No thread could start: plans run on the UI thread.
    inline: Option<Arc<Snapshot>>,
}

impl Matcher {
    fn start(ctx: &egui::Context, snap: Arc<Snapshot>) -> Self {
        let (tx, requests) = channel::<(u64, Settings)>();
        let (sent, rx) = channel();
        let latest = Arc::new(AtomicU64::new(0));
        let (ctx, newest, thread_snap) = (ctx.clone(), latest.clone(), snap.clone());
        let spawned = std::thread::Builder::new()
            .name("batch-rename".into())
            .spawn(move || {
                while let Ok(mut req) = requests.recv() {
                    while let Ok(newer) = requests.try_recv() {
                        req = newer;
                    }
                    let (n, s) = req;
                    let current = || newest.load(Ordering::Relaxed) == n;
                    plan_into(&thread_snap, &s, &mut |part| {
                        let ok = current() && sent.send(Sent::Part(n, part)).is_ok();
                        ctx.request_repaint();
                        ok
                    });
                    if current() && sent.send(Sent::Done(n)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        let inline = match spawned {
            Ok(_) => None,
            Err(e) => {
                log::warn!(target: "ondin", "no thread for batch rename: {e}");
                Some(snap)
            }
        };
        Self {
            tx,
            rx,
            latest,
            inline,
        }
    }

    /// Ask for the plan of `s`; its parts come back through [`Self::rx`] under the
    /// returned number.
    fn ask(&mut self, s: Settings) -> u64 {
        let n = self.latest.load(Ordering::Relaxed) + 1;
        self.latest.store(n, Ordering::Relaxed);
        match &self.inline {
            Some(snap) => {
                let whole = plan(snap, &s);
                // Sent to ourselves, so the frame reads it as it would the thread's.
                let (tx, rx) = channel();
                let _ = tx.send(Sent::Part(n, whole));
                let _ = tx.send(Sent::Done(n));
                self.rx = rx;
            }
            None => {
                let _ = self.tx.send((n, s));
            }
        }
        n
    }
}

impl BatchRename {
    /// Ask for a plan when the settings changed, and take in what the worker sent.
    fn take_parts(&mut self) {
        if self.asked.as_ref() != Some(&self.s) {
            self.matcher.ask(self.s.clone());
            self.asked = Some(self.s.clone());
            self.fresh = false;
            self.done = false;
        }
        let newest = self.matcher.latest.load(Ordering::Relaxed);
        while let Ok(sent) = self.matcher.rx.try_recv() {
            match sent {
                Sent::Part(n, part) if n == newest => {
                    if !self.fresh {
                        self.fresh = true;
                        self.plan = Plan::default();
                    }
                    self.plan.absorb(part);
                }
                Sent::Done(n) if n == newest => {
                    if !self.fresh {
                        self.fresh = true;
                        self.plan = Plan::default();
                    }
                    self.done = true;
                }
                _ => {}
            }
        }
    }
}

/// `n` with thousands separated: `1,204`.
fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The preview's count: what changes at Selection, what matches elsewhere.
fn count_label(scope: Scope, plan: &Plan) -> String {
    match scope {
        Scope::Selection => format!(
            "{} of {} change",
            count(plan.changes.len()),
            count(plan.searched)
        ),
        Scope::Page => format!("{} of {} match", count(plan.listed), count(plan.searched)),
        Scope::Document => format!(
            "{} of {} match · {} page{}",
            count(plan.listed),
            count(plan.searched),
            plan.pages,
            if plan.pages == 1 { "" } else { "s" }
        ),
    }
}

fn layers(n: usize) -> String {
    match n {
        1 => "1 layer".to_string(),
        n => format!("{} layers", count(n)),
    }
}

/// The template's token at char index `at` or ending there, as a char range.
fn token_at(text: &str, at: usize, ending: bool) -> Option<Range<usize>> {
    ondin_core::rename::tokens(text)
        .into_iter()
        .find_map(|(r, _)| {
            let start = text[..r.start].chars().count();
            let end = start + text[r.clone()].chars().count();
            let hit = match ending {
                true => start < at && at <= end,
                false => start <= at && at < end,
            };
            hit.then_some(start..end)
        })
}

fn char_to_byte(text: &str, at: usize) -> usize {
    text.char_indices().nth(at).map_or(text.len(), |(b, _)| b)
}

/// `text` with each typed spelling of a token written as the token, and `caret`
/// moved with the text before it.
fn canonical(text: &str, caret: usize) -> (String, usize) {
    let mut out = String::new();
    let mut moved = caret;
    let mut at = 0;
    for (r, piece) in ondin_core::rename::tokens(text) {
        out.push_str(&text[at..r.start]);
        let spelled = Template(vec![piece]).to_string();
        let end = text[..r.end].chars().count();
        if caret >= end {
            moved = moved + spelled.chars().count() - text[r.clone()].chars().count();
        }
        out.push_str(&spelled);
        at = r.end;
    }
    out.push_str(&text[at..]);
    (out, moved)
}

/// A pill's ground behind a token.
fn pill_fill() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(109, 140, 217, 66)
}

/// The template field's text: each token a pill, its brackets drawn as the pill's
/// padding so the caret still steps through real characters. The pill keeps the
/// text's size: a smaller font sits on the line's baseline, low against its words.
fn template_galley(ui: &egui::Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    let font = egui::FontId::proportional(12.0);
    let plain = egui::TextFormat::simple(font.clone(), color::TEXT);
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for (r, _) in ondin_core::rename::tokens(text) {
        job.append(&text[at..r.start], 0.0, plain.clone());
        let pill = |ink| egui::TextFormat {
            color: ink,
            background: pill_fill(),
            ..egui::TextFormat::simple(font.clone(), ink)
        };
        // The galley must hold exactly the text's characters, or the caret lands
        // between the wrong ones: the word is the text's own, the brackets hidden.
        let bracket = pill(egui::Color32::TRANSPARENT);
        job.append("[", 0.0, bracket.clone());
        job.append(&text[r.start + 1..r.end - 1], 0.0, pill(color::ACCENT_200));
        job.append("]", 0.0, bracket);
        at = r.end;
    }
    job.append(&text[at..], 0.0, plain);
    job.wrap.max_width = f32::INFINITY;
    ui.fonts_mut(|f| f.layout_job(job))
}

impl OndinApp {
    /// *Batch rename…*: open the box over the canvas. It opens on the selection
    /// when there is one and on the page when not; `search` is the layers tree's
    /// filter, which opens it on the page finding what the tree shows.
    pub(crate) fn open_batch_rename(&mut self, ctx: &egui::Context, search: Option<String>) {
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        self.picker = None;
        let doc = &self.session.doc;
        let selection: Vec<NodeId> = self
            .session
            .selection
            .ids()
            .iter()
            .copied()
            .filter(|id| !doc.is_page(*id) && doc.get(*id).is_some())
            .collect();
        let remembered = self.prefs.batch_rename.clone();
        // One layer with children: renaming the layer alone is Rename's job.
        let inside = matches!(selection.as_slice(), [id]
            if doc.get(*id).is_some_and(|n| !n.children().is_empty()));
        let mut s = Settings {
            scope: Scope::Selection,
            page: self.session.page,
            find: remembered.find,
            template: if remembered.template.is_empty() {
                "[Name]".to_string()
            } else {
                remembered.template
            },
            match_case: false,
            pattern: false,
            keep_case: false,
            numbering: Numbering {
                from: remembered.from,
                step: remembered.step,
                digits: remembered.digits.clamp(1, 3),
            },
            order: Order::from_key(&remembered.order),
            inside,
            inside_instances: false,
        };
        let search = search.filter(|s| !s.trim().is_empty());
        if selection.is_empty() || search.is_some() {
            s.set_scope(Scope::Page);
        }
        if let Some(text) = search {
            s.find = text.trim().to_string();
        }
        let snap = Arc::new(Snapshot::take(doc, &self.session.resolved, &selection));
        self.batch_rename = Some(BatchRename {
            s,
            selected: selection.len(),
            matcher: Matcher::start(ctx, snap),
            asked: None,
            plan: Plan::default(),
            fresh: false,
            done: false,
            caret: None,
            focus: Some(Caret::All),
        });
    }

    /// Rename every layer the plan changes, as one undo step, and remember the
    /// box's Find, template and numbering for next time. Called only when the plan
    /// is whole and changes something.
    fn commit_batch_rename(&mut self, b: &BatchRename) {
        let br = &mut self.prefs.batch_rename;
        br.find = b.s.find.clone();
        br.template = b.s.template.clone();
        br.from = b.s.numbering.from;
        br.step = b.s.numbering.step;
        br.digits = b.s.numbering.digits;
        br.order = b.s.order.key().to_string();
        self.prefs.save();
        let tx = Transaction(
            b.plan
                .changes
                .iter()
                .map(|(id, name)| Operation::SetName {
                    id: *id,
                    name: name.clone(),
                })
                .collect(),
        );
        self.session.name_step("Rename", icon::TEXT_AA);
        if self.session.commit(tx) {
            self.session.info(format!(
                "Renamed {} · Ctrl+Z to undo",
                layers(b.plan.changes.len())
            ));
        }
    }

    /// Draw the open box and answer this frame's keys and clicks. A modal with a
    /// clear backdrop, so the selection stays outlined: a click outside closes it
    /// and renames nothing.
    pub(crate) fn batch_rename_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut b) = self.batch_rename.take() else {
            return;
        };
        // The order list, open last frame, takes Escape, Enter and the click
        // outside that closes it.
        let popup = egui::Popup::is_any_open(ctx);
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if escape && !popup {
            return;
        }
        let enter = !popup && ctx.input(|i| i.key_pressed(egui::Key::Enter));
        b.take_parts();

        let screen = ctx.content_rect();
        let id = egui::Id::new("batch-rename");
        let area = egui::Modal::default_area(id).anchor(
            egui::Align2::CENTER_TOP,
            egui::vec2(
                canvas.center().x - screen.center().x,
                canvas.top() - screen.top() + TOP_GAP,
            ),
        );
        let why_not = match b.done {
            true => b.plan.why_not(&b.s.find),
            false => Some("Still matching…".to_string()),
        };
        let mut act = Act::None;
        let modal = egui::Modal::new(id)
            .area(area)
            .backdrop_color(egui::Color32::TRANSPARENT)
            .frame(
                egui::Frame::NONE
                    .fill(color::CARD)
                    .stroke(egui::Stroke::new(1.0, color::DIVIDER))
                    .corner_radius(egui::CornerRadius::same(8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 14],
                        blur: 30,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(102),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(W);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                if head(ui) {
                    act = Act::Cancel;
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: PAD as i8,
                        right: PAD as i8,
                        top: PAD as i8,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = GAP;
                        scope_row(ui, &mut b.s, b.selected);
                        find_row(ui, &mut b.s, b.plan.error.as_deref());
                        template_row(ui, &mut b);
                        if Template::parse(&b.s.template).has_number() {
                            number_row(ui, &mut b.s.numbering);
                            order_row(ui, &mut b.s.order);
                        }
                        checks(ui, &mut b.s, b.plan.skipped);
                    });
                ui.add_space(8.0);
                divider(ui);
                preview(ui, b.s.scope, &b.plan, b.done);
                divider(ui);
                if let Some(a) = foot(ui, b.plan.changes.len(), why_not.as_deref()) {
                    act = a;
                }
            });
        if modal.backdrop_response.clicked() && !popup {
            return;
        }
        if enter {
            act = Act::Rename;
        }
        match act {
            Act::Cancel => {}
            Act::Rename if why_not.is_none() => {
                self.commit_batch_rename(&b);
                ctx.request_repaint();
            }
            _ => self.batch_rename = Some(b),
        }
    }
}

enum Act {
    None,
    Rename,
    Cancel,
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::DIVIDER);
}

/// The title row; true when its close button is clicked.
fn head(ui: &mut egui::Ui) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 40.0), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + PAD, y),
        egui::Align2::LEFT_CENTER,
        icon::TEXT_AA,
        theme::icon_font(15.0),
        theme::text::DIM,
    );
    p.text(
        egui::pos2(rect.left() + PAD + 23.0, y),
        egui::Align2::LEFT_CENTER,
        "Batch rename",
        egui::FontId::proportional(13.0),
        color::TEXT,
    );
    let close = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 8.0 - 13.0, y),
        egui::vec2(26.0, 26.0),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(close));
    ui::icon_button(&mut child, icon::X, 26.0, 13.0, false, true)
        .on_hover_text("Cancel")
        .clicked()
}

/// A grid row: the label in its column, then `add` in the rest.
fn row<R>(ui: &mut egui::Ui, label: &str, h: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(LABEL_W, h), egui::Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(12.0),
            theme::text::MUTED,
        );
        ui.add_space(GAP);
        add(ui)
    })
    .inner
}

/// A segment of a segmented control: lit when `on`, with an optional count after
/// its word.
fn segment(ui: &mut egui::Ui, text: &str, sub: &str, on: bool, h: f32) -> egui::Response {
    let font = egui::FontId::proportional(11.5);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font.clone(), color::TEXT));
    let sub_g = (!sub.is_empty()).then(|| {
        ui.fonts_mut(|f| f.layout_no_wrap(sub.to_string(), font.clone(), theme::text::DIM))
    });
    let w = galley.size().x + sub_g.as_ref().map_or(0.0, |g| g.size().x + 5.0) + 18.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, 4.0, color::text_a(36));
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, color::text_a(15));
    }
    let ink = if on { color::TEXT } else { theme::text::MUTED };
    let x = rect.left() + 9.0;
    p.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        ink,
    );
    if let Some(g) = sub_g {
        p.galley(
            egui::pos2(
                x + galley.size().x + 5.0,
                rect.center().y - g.size().y / 2.0,
            ),
            g,
            theme::text::DIM,
        );
    }
    resp
}

/// A rounded well holding segments `h` tall.
fn well<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::NONE
        .fill(color::FIELD)
        .corner_radius(5)
        .inner_margin(2)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(add).inner
        })
        .inner
}

fn scope_row(ui: &mut egui::Ui, s: &mut Settings, selected: usize) {
    row(ui, "In", ROW_H, |ui| {
        well(ui, |ui| {
            let n = match selected {
                0 => String::new(),
                n => n.to_string(),
            };
            for (scope, label, sub) in [
                (Scope::Selection, "Selection", n.as_str()),
                (Scope::Page, "This page", ""),
                (Scope::Document, "All pages", ""),
            ] {
                let live = scope != Scope::Selection || selected > 0;
                let resp = ui
                    .add_enabled_ui(live, |ui| {
                        segment(ui, label, sub, s.scope == scope, ROW_H - 4.0)
                    })
                    .inner;
                let resp = if live {
                    resp
                } else {
                    resp.on_disabled_hover_text("Nothing is selected")
                };
                if resp.clicked() && s.scope != scope {
                    s.set_scope(scope);
                }
            }
        });
    });
}

/// A toggle inside a field: `Aa`, `.*`, `Ab`.
fn field_toggle(ui: &mut egui::Ui, text: &str, tip: &str, on: &mut bool, mono: bool) {
    let font = match mono {
        true => egui::FontId::monospace(11.0),
        false => egui::FontId::proportional(11.0),
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(24.0, 22.0), egui::Sense::click());
    let p = ui.painter();
    if *on {
        p.rect_filled(rect, 4.0, pill_fill());
    } else if resp.hovered() {
        p.rect_filled(rect, 4.0, color::text_a(15));
    }
    let ink = if *on {
        color::ACCENT_200
    } else {
        theme::text::DIM
    };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, font, ink);
    if resp.on_hover_text(tip).clicked() {
        *on = !*on;
    }
}

/// A field's well and ring, lit while `focused`.
fn field_ground(ui: &egui::Ui, rect: egui::Rect, focused: bool) {
    let p = ui.painter();
    p.rect_filled(rect, 5.0, color::FIELD);
    let ring = if focused {
        egui::Stroke::new(1.0, color::ACCENT_400)
    } else {
        egui::Stroke::new(1.0, color::text_a(18))
    };
    p.rect_stroke(rect, 5.0, ring, egui::StrokeKind::Inside);
}

const FIND_ID: &str = "batch-rename-find";
const TEMPLATE_ID: &str = "batch-rename-template";

/// The part of a field `rect` its text takes, `right` short of its right edge for
/// the toggles there.
fn text_rect(rect: egui::Rect, right: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        rect.min + egui::vec2(9.0, 0.0),
        egui::pos2(rect.right() - 4.0 - right, rect.bottom()),
    )
}

/// The toggles at a field's right end, `w` wide.
fn toggles(ui: &mut egui::Ui, rect: egui::Rect, w: f32) -> egui::Ui {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_max(
                egui::pos2(rect.right() - 4.0 - w, rect.top()),
                egui::pos2(rect.right() - 4.0, rect.bottom()),
            ))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 4.0;
    child
}

fn find_row(ui: &mut egui::Ui, s: &mut Settings, error: Option<&str>) {
    row(ui, "Find", FIELD_H, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, FIELD_H), egui::Sense::hover());
            let id = egui::Id::new(FIND_ID);
            field_ground(ui, rect, ui.memory(|m| m.has_focus(id)));
            let text = text_rect(rect, 52.0);
            ui.put(
                text,
                egui::TextEdit::singleline(&mut s.find)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .vertical_align(egui::Align::Center)
                    .desired_width(text.width())
                    .font(egui::FontId::proportional(12.0))
                    .text_color(color::TEXT)
                    .hint_text(
                        egui::RichText::new("Whole name")
                            .size(12.0)
                            .color(theme::text::FAINT),
                    ),
            );
            let mut child = toggles(ui, rect, 52.0);
            field_toggle(&mut child, "Aa", "Match case", &mut s.match_case, false);
            field_toggle(&mut child, ".*", "Find as a pattern", &mut s.pattern, true);
            if let Some(e) = error {
                ui.label(egui::RichText::new(e).size(10.5).color(color::WARN));
            }
        });
    });
}

/// Rename to: the template field with its tokens as pills, and the buttons that
/// insert them.
fn template_row(ui: &mut egui::Ui, b: &mut BatchRename) {
    let groups = b.plan.groups;
    row(ui, "Rename to", FIELD_H, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 7.0;
            let w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, FIELD_H), egui::Sense::hover());
            let id = egui::Id::new(TEMPLATE_ID);
            let focused = ui.memory(|m| m.has_focus(id));
            field_ground(ui, rect, focused);

            // Backspace and Delete take a pill out whole: read before the field,
            // which would take one bracket.
            if focused {
                take_pill(ui, id, &mut b.s.template);
            }
            let text = text_rect(rect, 24.0);
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                template_galley(ui, buf.as_str())
            };
            // `put` into the field's whole height, as Find is: shown in a child Ui,
            // the edit takes one line's height and its text rides at the top.
            let resp = ui.put(
                text,
                egui::TextEdit::singleline(&mut b.s.template)
                    .id(id)
                    .frame(egui::Frame::NONE)
                    .margin(egui::Margin::ZERO)
                    .vertical_align(egui::Align::Center)
                    .desired_width(text.width())
                    .font(egui::FontId::proportional(12.0))
                    .layouter(&mut layouter),
            );
            let cursor =
                egui::TextEdit::load_state(ui.ctx(), id).and_then(|s| s.cursor.char_range());
            let caret = cursor.map(|c| c.primary.index.0);
            if resp.changed() {
                let (text, moved) = canonical(&b.s.template, caret.unwrap_or(0));
                if text != b.s.template {
                    b.s.template = text;
                    b.focus = Some(Caret::At(moved));
                }
            }
            if resp.has_focus() {
                b.caret = caret;
                // A caret inside a pill steps out of it, to its far side.
                if let Some(c) = cursor
                    && c.primary.index == c.secondary.index
                    && let Some(r) = token_at(&b.s.template, c.primary.index.0, false)
                    && r.start < c.primary.index.0
                {
                    b.focus = Some(Caret::At(r.end));
                }
            }
            if let Some(f) = b.focus.take() {
                let len = b.s.template.chars().count();
                let range = match f {
                    Caret::All => egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(len),
                    ),
                    Caret::At(at) => {
                        egui::text::CCursorRange::one(egui::text::CCursor::new(at.min(len)))
                    }
                };
                let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                state.cursor.set_char_range(Some(range));
                state.store(ui.ctx(), id);
                ui.memory_mut(|m| m.request_focus(id));
            }
            let mut child = toggles(ui, rect, 24.0);
            field_toggle(
                &mut child,
                "Ab",
                "Keep each match’s case",
                &mut b.s.keep_case,
                false,
            );

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let mut insert = |ui: &mut egui::Ui, label: &str, piece: Piece| {
                    if insert_button(ui, label).clicked() {
                        let token = Template(vec![piece]).to_string();
                        let len = b.s.template.chars().count();
                        let at = b.caret.unwrap_or(len).min(len);
                        let byte = char_to_byte(&b.s.template, at);
                        b.s.template.insert_str(byte, &token);
                        b.focus = Some(Caret::At(at + token.chars().count()));
                    }
                };
                insert(ui, "Name", Piece::Name);
                insert(ui, "Number", Piece::Number);
                for k in 1..=groups.min(9) {
                    insert(ui, &format!("Group {k}"), Piece::Group(k));
                }
                if b.s.template.is_empty() && !b.s.find.is_empty() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("Empty: removes the match")
                                .size(10.5)
                                .color(theme::text::FAINT),
                        );
                    });
                }
            });
        });
    });
}

/// Take the pill before the caret on Backspace, or after it on Delete.
fn take_pill(ui: &egui::Ui, id: egui::Id, text: &mut String) {
    let Some(state) = egui::TextEdit::load_state(ui.ctx(), id) else {
        return;
    };
    let Some(range) = state.cursor.char_range() else {
        return;
    };
    if range.primary.index != range.secondary.index {
        return;
    }
    let at = range.primary.index.0;
    let none = egui::Modifiers::NONE;
    let hit = ui.input_mut(|i| {
        if let Some(r) = token_at(text, at, true)
            && i.consume_key(none, egui::Key::Backspace)
        {
            return Some(r);
        }
        if let Some(r) = token_at(text, at, false)
            && i.consume_key(none, egui::Key::Delete)
        {
            return Some(r);
        }
        None
    });
    if let Some(r) = hit {
        let (a, z) = (char_to_byte(text, r.start), char_to_byte(text, r.end));
        text.replace_range(a..z, "");
        let mut state = state;
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(r.start),
            )));
        state.store(ui.ctx(), id);
    }
}

fn insert_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let font = egui::FontId::proportional(11.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font, theme::text::MUTED));
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 7.0 * 2.0 + 14.0, 22.0),
        egui::Sense::click(),
    );
    let p = ui.painter();
    p.rect_filled(
        rect,
        4.0,
        color::text_a(if resp.hovered() { 26 } else { 15 }),
    );
    p.text(
        egui::pos2(rect.left() + 7.0 + 5.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::PLUS,
        theme::icon_font(10.0),
        theme::text::MUTED,
    );
    p.galley(
        egui::pos2(
            rect.left() + 7.0 + 14.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        theme::text::MUTED,
    );
    resp.on_hover_text(format!("Insert {label} at the caret"))
}

/// *From* and *Step* as the inspector's scrubbable fields, then the digits.
fn number_row(ui: &mut egui::Ui, n: &mut Numbering) {
    row(ui, "Number", NUM_H, |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let size = egui::vec2(96.0, NUM_H);
        // 0.5, the rate every other whole-unit field scrubs at.
        ui::value_field(
            ui,
            size,
            Prefix::Text("From"),
            &mut n.from,
            Scrub::whole(0.5),
            |d| d.custom_formatter(ui::number(0)),
        )
        .on_hover_text("The first layer's number");
        ui::value_field(
            ui,
            size,
            Prefix::Text("Step"),
            &mut n.step,
            Scrub::whole(0.5),
            |d| d.custom_formatter(ui::number(0)),
        )
        .on_hover_text("Added for each layer after it; below zero counts down");
        well(ui, |ui| {
            for (digits, label) in [(1u8, "1"), (2, "01"), (3, "001")] {
                if segment(ui, label, "", n.digits == digits, NUM_H - 4.0)
                    .on_hover_text(match digits {
                        1 => "No padding".to_string(),
                        d => format!("Pad to {d} digits"),
                    })
                    .clicked()
                {
                    n.digits = digits;
                }
            }
        });
    });
}

/// Which layer of the run is first, as the Settings modal's dropdowns.
fn order_row(ui: &mut egui::Ui, order: &mut Order) {
    row(ui, "Order", NUM_H, |ui| {
        ui.spacing_mut().interact_size.y = NUM_H;
        ui.spacing_mut().button_padding.y = 0.0;
        egui::ComboBox::from_id_salt("batch-rename-order")
            .icon(ui::combo_chevron)
            .width(200.0)
            .selected_text(ui::glyph_and_text(order.glyph(), order.label()))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                ui.spacing_mut().button_padding.y = 2.0;
                for o in Order::ALL {
                    ui.selectable_value(order, o, ui::glyph_and_text(o.glyph(), o.label()));
                }
            })
            .response
            .on_hover_text("Which layer is first");
    });
}

fn checkbox(ui: &mut egui::Ui, label: &str, note: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W + GAP);
        let font = egui::FontId::proportional(12.0);
        let galley = ui
            .fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), theme::text::STRONG));
        let note_g = ui.fonts_mut(|f| f.layout_no_wrap(note.to_string(), font, theme::text::FAINT));
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(14.0 + 7.0 + galley.size().x + 5.0 + note_g.size().x, 24.0),
            egui::Sense::click(),
        );
        let p = ui.painter();
        let tick = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.center().y - 7.0),
            egui::vec2(14.0, 14.0),
        );
        if *on {
            p.rect_filled(tick, 3.0, color::ACCENT);
            p.text(
                tick.center(),
                egui::Align2::CENTER_CENTER,
                icon::CHECK,
                theme::icon_font(10.0),
                egui::Color32::WHITE,
            );
        } else {
            let ring = if resp.hovered() {
                color::text_a(140)
            } else {
                color::text_a(90)
            };
            p.rect_stroke(
                tick,
                3.0,
                egui::Stroke::new(1.0, ring),
                egui::StrokeKind::Inside,
            );
        }
        let x = tick.right() + 7.0;
        let w = galley.size().x;
        p.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            theme::text::STRONG,
        );
        p.galley(
            egui::pos2(x + w + 5.0, rect.center().y - note_g.size().y / 2.0),
            note_g,
            theme::text::FAINT,
        );
        if resp.clicked() {
            *on = !*on;
        }
    });
}

fn checks(ui: &mut egui::Ui, s: &mut Settings, skipped: usize) {
    match s.scope {
        Scope::Selection => checkbox(ui, "Include layers inside", "", &mut s.inside),
        Scope::Page | Scope::Document => {
            let note = match skipped {
                0 => String::new(),
                n => format!("· {} skipped", count(n)),
            };
            checkbox(
                ui,
                "Include layers inside instances",
                &note,
                &mut s.inside_instances,
            );
        }
    }
}

/// The preview: every layer at Selection, the matches elsewhere. Only the rows in
/// view are laid out, so a run of thousands costs what eight do.
fn preview(ui: &mut egui::Ui, scope: Scope, plan: &Plan, done: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 30.0), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.bottom() - 10.0;
    p.text(
        egui::pos2(rect.left() + PAD, y),
        egui::Align2::LEFT_CENTER,
        "PREVIEW",
        egui::FontId::proportional(9.5),
        theme::text::FAINT,
    );
    p.text(
        egui::pos2(rect.right() - PAD, y),
        egui::Align2::RIGHT_CENTER,
        count_label(scope, plan),
        egui::FontId::proportional(10.5),
        theme::text::DIM,
    );
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 6,
            right: 6,
            top: 0,
            bottom: 8,
        })
        .show(ui, |ui| {
            if plan.rows.is_empty() {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
                ui.painter().text(
                    rect.left_center() + egui::vec2(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    if done {
                        "No layer matches"
                    } else {
                        "Matching…"
                    },
                    egui::FontId::proportional(12.0),
                    theme::text::DIM,
                );
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("batch-rename-preview")
                .max_height(ROW_H * ROWS_SHOWN)
                .auto_shrink([false, true])
                .show_rows(ui, ROW_H, plan.rows.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for r in &plan.rows[range] {
                        match r {
                            Row::Page { name, count } => page_head(ui, name, *count),
                            Row::Layer(l) => layer_row(ui, l),
                        }
                    }
                });
        });
}

fn page_head(ui: &mut egui::Ui, name: &str, n: usize) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.bottom() - 4.0 - 6.0;
    p.text(
        egui::pos2(rect.left() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        icon::FILE,
        theme::icon_font(11.0),
        theme::text::DIM,
    );
    p.text(
        egui::pos2(rect.left() + 8.0 + 17.0, y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(10.5),
        theme::text::DIM,
    );
    p.text(
        egui::pos2(rect.right() - 8.0, y),
        egui::Align2::RIGHT_CENTER,
        n.to_string(),
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
}

fn layer_row(ui: &mut egui::Ui, l: &LayerRow) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
    let dim = matches!(l.new, New::Unchanged);
    let fade = |c: egui::Color32| if dim { c.gamma_multiply(0.42) } else { c };
    let p = ui.painter();
    let y = rect.center().y;
    let inner = rect.shrink2(egui::vec2(8.0, 0.0));
    // 16 | old 1fr | 14 | new 1.15fr, 8 apart.
    let rest = inner.width() - 16.0 - 14.0 - 8.0 * 3.0;
    let old_w = rest / 2.15;
    let new_w = rest - old_w;
    let x_old = inner.left() + 16.0 + 8.0;
    let x_arrow = x_old + old_w + 8.0;
    let x_new = x_arrow + 14.0 + 8.0;
    p.text(
        egui::pos2(inner.left() + 8.0, y),
        egui::Align2::CENTER_CENTER,
        l.glyph,
        theme::icon_font(14.0),
        fade(l.tint),
    );

    let font = egui::FontId::proportional(12.0);
    let mut old = egui::text::LayoutJob::default();
    let mut at = 0;
    let plain = egui::TextFormat::simple(font.clone(), fade(theme::text::MUTED));
    for m in &l.matched {
        if m.start < at || m.end > l.old.len() {
            continue;
        }
        old.append(&l.old[at..m.start], 0.0, plain.clone());
        old.append(
            &l.old[m.clone()],
            0.0,
            egui::TextFormat {
                color: fade(theme::text::STRONG),
                underline: egui::Stroke::new(1.0, fade(theme::text::MUTED)),
                ..egui::TextFormat::simple(font.clone(), color::TEXT)
            },
        );
        at = m.end;
    }
    old.append(&l.old[at..], 0.0, plain);
    old.wrap = egui::text::TextWrapping::truncate_at_width(old_w);
    let old = ui.fonts_mut(|f| f.layout_job(old));
    p.galley(egui::pos2(x_old, y - old.size().y / 2.0), old, color::TEXT);

    p.text(
        egui::pos2(x_arrow + 7.0, y),
        egui::Align2::CENTER_CENTER,
        icon::ARROW_RIGHT,
        theme::icon_font(11.0),
        fade(theme::text::FAINT),
    );

    let mut new = egui::text::LayoutJob::default();
    let fmt = |c| egui::TextFormat::simple(font.clone(), c);
    match &l.new {
        New::Name(spans) => {
            for s in spans {
                let c = if s.changed {
                    color::ACCENT_300
                } else {
                    color::TEXT
                };
                new.append(&s.text, 0.0, fmt(c));
            }
        }
        New::Unchanged => new.append(&l.old, 0.0, fmt(fade(theme::text::DIM))),
        New::Empty => new.append("Would be empty · kept", 0.0, fmt(color::WARN)),
        New::Variant => new.append("Named by its properties", 0.0, fmt(color::WARN)),
    }
    new.wrap = egui::text::TextWrapping::truncate_at_width(new_w);
    let new = ui.fonts_mut(|f| f.layout_job(new));
    p.galley(egui::pos2(x_new, y - new.size().y / 2.0), new, color::TEXT);
}

/// The footer: the hint, then *Cancel* and *Rename* as the Settings modal's
/// footer draws them. `why_not` dims Rename and is its tooltip.
fn foot(ui: &mut egui::Ui, changes: usize, why_not: Option<&str>) -> Option<Act> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 48.0), egui::Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() + PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Enter renames · Esc cancels",
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    let go = match changes {
        0 => "Rename".to_string(),
        n => format!("Rename {}", layers(n)),
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 10.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    let state = match why_not {
        None => FieldButton::On,
        Some(_) => FieldButton::Disabled,
    };
    let primary = crate::settings::text_button(&mut child, &go, state);
    let primary = match why_not {
        Some(why) => primary.on_hover_text(why),
        None => primary,
    };
    let cancel = crate::settings::text_button(&mut child, "Cancel", FieldButton::Off);
    if primary.clicked() && why_not.is_none() {
        Some(Act::Rename)
    } else if cancel.clicked() {
        Some(Act::Cancel)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{IdSource, NodeKind};

    /// A `size` rectangle called `name` at `at` on the current page, on top.
    fn rect(app: &mut OndinApp, name: &str, at: (f64, f64), size: (f64, f64)) -> NodeId {
        let s = &mut app.session;
        let id = s.ids.mint();
        let parent = s.page;
        let index = s.doc.get(parent).map_or(0, |n| n.children().len());
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(size.0, size.1),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(at)),
            name: Some(name.into()),
        }])));
        id
    }

    fn named(app: &mut OndinApp, name: &str) -> NodeId {
        rect(app, name, (0.0, 0.0), (10.0, 10.0))
    }

    fn app() -> (egui::Context, OndinApp) {
        let ctx = egui::Context::default();
        let app = OndinApp::headless(&ctx);
        (ctx, app)
    }

    /// The box as `open_batch_rename` leaves it, with Find and the template set.
    fn opened(ctx: &egui::Context, app: &mut OndinApp, find: &str, template: &str) -> BatchRename {
        app.open_batch_rename(ctx, None);
        let mut b = app.batch_rename.take().expect("the box opened");
        b.s.find = find.into();
        b.s.template = template.into();
        b
    }

    /// What the box opened over: the snapshot its worker plans from.
    fn snapshot(app: &OndinApp) -> Snapshot {
        let s = &app.session;
        Snapshot::take(&s.doc, &s.resolved, s.selection.ids())
    }

    fn planned(app: &OndinApp, b: &BatchRename) -> Plan {
        plan(&snapshot(app), &b.s)
    }

    fn new_names(app: &OndinApp, plan: &Plan) -> Vec<(String, String)> {
        plan.changes
            .iter()
            .map(|(id, new)| {
                let old = app.session.doc.get(*id).unwrap().name().to_string();
                (old, new.clone())
            })
            .collect()
    }

    /// The run is the tree's order, top row first, and only a layer that changes
    /// takes a number: the unmatched one is listed, dimmed, and leaves no gap.
    ///
    /// flip: `plan_into` counting `taken` for every listed layer → fails at the
    /// second name, `Item 3 a`.
    #[test]
    fn a_run_is_numbered_in_tree_order_and_a_skip_leaves_no_gap() {
        let (ctx, mut app) = app();
        let a = named(&mut app, "Rect a");
        let skip = named(&mut app, "Other");
        let b = named(&mut app, "Rect b");
        app.session.selection.set(vec![a, skip, b]);
        let bx = opened(&ctx, &mut app, "Rect", "Item [Number]");
        assert_eq!(bx.s.scope, Scope::Selection);
        let plan = planned(&app, &bx);
        assert_eq!(
            new_names(&app, &plan),
            [
                ("Rect b".to_string(), "Item 1 b".to_string()),
                ("Rect a".to_string(), "Item 2 a".to_string()),
            ]
        );
        assert_eq!(plan.listed, 3, "Selection lists every layer");
        let Row::Layer(middle) = &plan.rows[1] else {
            panic!("a layer row")
        };
        assert_eq!((middle.id, &middle.new), (skip, &New::Unchanged));
        assert_eq!(count_label(bx.s.scope, &plan), "2 of 3 change");
    }

    /// *Left to right* takes rows by band, though no two tops line up, then reads
    /// each row left to right; *Top to bottom* takes columns the same way.
    ///
    /// flip: `by_bands` putting every layer in one band (sorting by the left edge
    /// alone) → fails at the left-to-right order. Joining a band by the top rather
    /// than the centre gives the same answer here.
    #[test]
    fn the_canvas_orders_read_by_band() {
        let (ctx, mut app) = app();
        // Made out of order, so the tree's order is not the answer.
        let cards = [
            ("e", (114.0, 104.0), 64.0),
            ("a", (16.0, 30.0), 52.0),
            ("f", (188.0, 124.0), 50.0),
            ("c", (190.0, 38.0), 44.0),
            ("d", (18.0, 118.0), 56.0),
            ("b", (116.0, 14.0), 60.0),
        ];
        let ids: Vec<NodeId> = cards
            .iter()
            .map(|(n, at, h)| rect(&mut app, n, *at, (76.0, *h)))
            .collect();
        app.session.selection.set(ids);
        let mut bx = opened(&ctx, &mut app, "", "[Name][Number]");
        bx.s.order = Order::LeftToRight;
        let got: Vec<String> = planned(&app, &bx)
            .changes
            .into_iter()
            .map(|c| c.1)
            .collect();
        assert_eq!(got, ["a1", "b2", "c3", "d4", "e5", "f6"]);
        bx.s.order = Order::TopToBottom;
        let got: Vec<String> = planned(&app, &bx)
            .changes
            .into_iter()
            .map(|c| c.1)
            .collect();
        assert_eq!(got, ["a1", "d2", "b3", "e4", "c5", "f6"]);
    }

    /// A name that would come out empty is kept and says so; it takes no number
    /// and is not counted in the button.
    #[test]
    fn an_empty_result_keeps_its_name() {
        let (ctx, mut app) = app();
        let hero = named(&mut app, "Draft Hero");
        let bare = named(&mut app, "Draft");
        app.session.selection.set(vec![hero, bare]);
        let bx = opened(&ctx, &mut app, "Draft", "");
        let plan = planned(&app, &bx);
        assert_eq!(
            new_names(&app, &plan),
            [("Draft Hero".to_string(), "Hero".to_string())]
        );
        assert!(
            plan.rows
                .iter()
                .any(|r| matches!(r, Row::Layer(l) if l.id == bare && l.new == New::Empty))
        );
    }

    /// A pattern that does not parse renames nothing and says why, which is also
    /// Rename's tooltip.
    #[test]
    fn a_bad_pattern_renames_nothing() {
        let (ctx, mut app) = app();
        let a = named(&mut app, "Card");
        app.session.selection.set_one(a);
        let mut bx = opened(&ctx, &mut app, "(", "x");
        bx.s.pattern = true;
        let plan = planned(&app, &bx);
        assert!(plan.changes.is_empty());
        let why = plan.why_not("(").expect("a reason");
        assert!(why.starts_with("Not a pattern"), "{why}");
        assert_eq!(plan.error.as_deref(), Some(why.as_str()));
    }

    /// Rename's tooltip says why it is dim: nothing matched, or every match would
    /// come out empty or the same.
    #[test]
    fn a_dim_rename_says_why() {
        let (ctx, mut app) = app();
        let a = named(&mut app, "Card");
        app.session.selection.set_one(a);
        let mut bx = opened(&ctx, &mut app, "zzz", "x");
        assert_eq!(
            planned(&app, &bx).why_not("zzz").as_deref(),
            Some("No layer matches Find")
        );
        bx.s.find = "Card".into();
        bx.s.template = "".into();
        assert_eq!(
            planned(&app, &bx).why_not("Card").as_deref(),
            Some("Every new name would be empty")
        );
        bx.s.template = "[Name]".into();
        assert_eq!(
            planned(&app, &bx).why_not("Card").as_deref(),
            Some("Nothing would change")
        );
        bx.s.template = "Tile".into();
        assert_eq!(planned(&app, &bx).why_not("Card"), None);
    }

    /// Main frame *Card* holding two rectangles, and an instance of it on the page.
    fn with_an_instance() -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xBA7C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [m, a, b] = [(); 3].map(|_| ids.mint());
        let create = |id, parent, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        let rect = || NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        doc.apply(&Transaction(vec![
            create(
                m,
                root,
                NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                "Card",
            ),
            create(a, m, rect(), "Button a"),
            create(b, m, rect(), "Button b"),
        ]))
        .expect("a frame holding two rects");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(m);
        app.create_component();
        app.duplicate_selection();
        app.session.selection.clear();
        (ctx, app, m)
    }

    /// At page scope the layers inside an instance are left out and counted, and
    /// *Include layers inside instances* takes them in. Keep case is on there.
    ///
    /// flip: `plan_into` ignoring `inside_instances` → fails at "the copies are
    /// left out".
    #[test]
    fn the_page_leaves_instance_insides_out() {
        let (ctx, mut app, _) = with_an_instance();
        let mut bx = opened(&ctx, &mut app, "button", "cta");
        assert_eq!(
            bx.s.scope,
            Scope::Page,
            "nothing selected opens on the page"
        );
        assert!(bx.s.keep_case, "keep case is on away from the selection");
        let plan = planned(&app, &bx);
        assert_eq!(plan.listed, 2, "the copies are left out");
        assert_eq!(plan.skipped, 2);
        assert!(
            plan.changes.iter().all(|c| c.1.starts_with("Cta ")),
            "{:?}",
            plan.changes
        );
        bx.s.inside_instances = true;
        let plan = planned(&app, &bx);
        assert_eq!((plan.listed, plan.skipped), (4, 0));
    }

    /// All pages heads each page's rows with its name and count, in the Pages
    /// list's order, and the number runs on across pages.
    #[test]
    fn all_pages_heads_each_page_and_numbers_on() {
        let (ctx, mut app) = app();
        named(&mut app, "Hero");
        named(&mut app, "Footer");
        let first = app.session.page;
        app.add_page();
        named(&mut app, "hero image");
        app.go_to_page(first);
        let mut bx = opened(&ctx, &mut app, "hero", "[Name] [Number]");
        bx.s.set_scope(Scope::Document);
        let plan = planned(&app, &bx);
        let heads: Vec<usize> = plan
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Page { count, .. } => Some(*count),
                Row::Layer(_) => None,
            })
            .collect();
        assert_eq!(heads, [1, 1]);
        let got: Vec<String> = plan.changes.into_iter().map(|c| c.1).collect();
        assert_eq!(got, ["Hero 1", "hero 2 image"]);
    }

    /// A plan sent in parts adds up to the plan made at once, its page headings
    /// counting rows that arrive in a later part.
    ///
    /// flip: `Plan::absorb` not counting a row into the heading before it → fails
    /// at the headings.
    #[test]
    fn parts_add_up_to_the_whole() {
        let (ctx, mut app) = app();
        for i in 0..(PART * 2 + 7) {
            named(&mut app, &format!("Tile {i}"));
        }
        let mut bx = opened(&ctx, &mut app, "Tile", "Card");
        bx.s.set_scope(Scope::Document);
        let mut parts = 0;
        let mut whole = Plan::default();
        plan_into(&snapshot(&app), &bx.s, &mut |part| {
            parts += 1;
            whole.absorb(part);
            true
        });
        assert_eq!(parts, 3);
        assert_eq!(whole.listed, PART * 2 + 7);
        assert_eq!(whole.changes.len(), PART * 2 + 7);
        let Some(Row::Page { count, .. }) = whole.rows.first() else {
            panic!("all pages heads the page")
        };
        assert_eq!(*count, PART * 2 + 7);
        let at_once = plan(&snapshot(&app), &bx.s);
        assert_eq!(whole.searched, at_once.searched);
        assert_eq!(whole.rows, at_once.rows);
        assert!(count_label(Scope::Document, &whole).starts_with("1,007 of "));
    }

    /// The count over the preview, at document scope.
    #[test]
    fn the_count_names_matches_and_pages() {
        let plan = Plan {
            listed: 5,
            searched: 1204,
            pages: 3,
            ..Plan::default()
        };
        assert_eq!(
            count_label(Scope::Document, &plan),
            "5 of 1,204 match · 3 pages"
        );
        assert_eq!(count_label(Scope::Page, &plan), "5 of 1,204 match");
    }

    /// Opening: one layer with children ticks *Include layers inside*; the layers
    /// search opens on the page with Find holding its text.
    #[test]
    fn the_box_opens_on_what_fits() {
        let (ctx, mut app, m) = with_an_instance();
        app.session.selection.set_one(m);
        app.open_batch_rename(&ctx, None);
        let b = app.batch_rename.take().unwrap();
        assert_eq!(
            (b.s.scope, b.s.inside, b.s.keep_case),
            (Scope::Selection, true, false)
        );
        app.open_batch_rename(&ctx, Some(" button ".into()));
        let b = app.batch_rename.take().unwrap();
        assert_eq!((b.s.scope, b.s.find.as_str()), (Scope::Page, "button"));
    }

    /// Typed short forms become their tokens with the caret moved past them, and
    /// a pill is found from either side for Backspace and Delete.
    #[test]
    fn tokens_are_spelled_out_and_found_whole() {
        assert_eq!(canonical("Card [n]", 8), ("Card [Number]".to_string(), 13));
        assert_eq!(canonical("[name] x", 2), ("[Name] x".to_string(), 2));
        assert_eq!(token_at("Card [Number]", 13, true), Some(5..13));
        assert_eq!(token_at("Card [Number]", 5, true), None);
        assert_eq!(token_at("Card [Number]", 5, false), Some(5..13));
    }

    /// One frame of the whole app.
    fn pass(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            modifiers,
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
    }

    /// Frames until the worker's plan for the box's settings is whole.
    fn settle(ctx: &egui::Context, app: &mut OndinApp) {
        let start = std::time::Instant::now();
        loop {
            pass(ctx, app, Vec::new(), egui::Modifiers::NONE);
            if app.batch_rename.as_ref().is_none_or(|b| b.done) {
                return;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(10),
                "the plan never landed"
            );
            std::thread::yield_now();
        }
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

    const CTRL: egui::Modifiers = egui::Modifiers {
        alt: false,
        ctrl: true,
        shift: false,
        mac_cmd: false,
        command: true,
    };

    /// Through real frames: `Ctrl+R` on two layers opens the box, typing replaces
    /// the selected Name pill and `[n]` becomes a Number pill, and Enter renames
    /// both as one undo step and remembers the template.
    ///
    /// flip: `rename_selection` without its two-or-more branch → fails at "Ctrl+R
    /// opens the box".
    #[test]
    fn ctrl_r_on_two_layers_renames_them_in_one_step() {
        let (ctx, mut app) = app();
        let none = egui::Modifiers::NONE;
        let a = named(&mut app, "Rectangle 1");
        let b = named(&mut app, "Rectangle 2");
        app.session.selection.set(vec![a, b]);
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, vec![key(egui::Key::R, CTRL)], CTRL);
        assert!(app.batch_rename.is_some(), "Ctrl+R opens the box");
        // A frame for the field to take focus and select its pill.
        pass(&ctx, &mut app, Vec::new(), none);
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text("Card [n]".into())],
            none,
        );
        assert_eq!(
            app.batch_rename.as_ref().map(|b| b.s.template.as_str()),
            Some("Card [Number]"),
            "typing replaced the Name pill"
        );
        settle(&ctx, &mut app);
        let depth = app.session.history.undo_depth();
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, none)], none);
        assert!(app.batch_rename.is_none(), "Enter closes the box");
        let name = |app: &OndinApp, id| app.session.doc.get(id).unwrap().name().to_string();
        assert_eq!(
            (name(&app, b), name(&app, a)),
            ("Card 1".into(), "Card 2".into())
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
        assert_eq!(app.prefs.batch_rename.template, "Card [Number]");
        assert!(app.session.undo());
        assert_eq!(name(&app, a), "Rectangle 1");
        assert_eq!(name(&app, b), "Rectangle 2");
    }

    /// Backspace takes a pill out whole, and Escape closes the box renaming nothing.
    #[test]
    fn backspace_takes_a_pill_and_escape_cancels() {
        let (ctx, mut app) = app();
        let none = egui::Modifiers::NONE;
        let a = named(&mut app, "Rectangle 1");
        app.session.selection.set_one(a);
        pass(&ctx, &mut app, Vec::new(), none);
        app.open_batch_rename(&ctx, None);
        pass(&ctx, &mut app, Vec::new(), none);
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text("x[number]".into())],
            none,
        );
        pass(&ctx, &mut app, Vec::new(), none);
        assert_eq!(app.batch_rename.as_ref().unwrap().s.template, "x[Number]");
        pass(&ctx, &mut app, vec![key(egui::Key::Backspace, none)], none);
        assert_eq!(
            app.batch_rename.as_ref().unwrap().s.template,
            "x",
            "the pill went whole"
        );
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, none)], none);
        assert!(app.batch_rename.is_none());
        assert_eq!(app.session.doc.get(a).unwrap().name(), "Rectangle 1");
    }

    /// The template's text, pills included, is centred in its field: the text's
    /// vertical middle within a point of the field's.
    ///
    /// flip: showing the template edit in a child `Ui` instead of `ui.put` → fails,
    /// the text riding at the field's top.
    #[test]
    fn the_template_text_is_centred_in_its_field() {
        let (ctx, mut app) = app();
        let none = egui::Modifiers::NONE;
        let a = named(&mut app, "Rectangle 1");
        app.session.selection.set_one(a);
        pass(&ctx, &mut app, Vec::new(), none);
        app.open_batch_rename(&ctx, None);
        let mut out = pass(&ctx, &mut app, Vec::new(), none);
        for _ in 0..3 {
            out = pass(&ctx, &mut app, Vec::new(), none);
        }
        let text = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == "[Name]" => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .expect("the template's galley");
        // The field's ground: the well painted under the text.
        let field = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                // By shape, not fill: the modal fades in, tinting its grounds.
                egui::Shape::Rect(r)
                    if (r.rect.height() - FIELD_H).abs() < 0.5
                        && r.rect.contains(text.left_center() + egui::vec2(1.0, 0.0)) =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no field under the text at {text:?}"));
        assert!(
            (text.center().y - field.center().y).abs() < 1.0,
            "text {text:?} in field {field:?}"
        );
    }
}
