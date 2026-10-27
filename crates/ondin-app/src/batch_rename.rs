//! Batch rename (`docs/design/app-canvas-chrome.md`, *Batch rename*): the box
//! that renames many layers at once — the selection, the page or every page.
//!
//! The text rule is `ondin_core::rename`'s; this module picks the layers, orders
//! and numbers them ([`plan`]), draws the box with its preview, and commits every
//! new name as one undo step. Nothing changes on the canvas until then.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::rename::{Find, Numbering, Piece, Rule, Span, Template};
use ondin_core::{Document, NodeId, Operation, Resolved, Transaction};
use std::collections::HashSet;
use std::ops::Range;

/// The box's width, and the gap between the top of the canvas and the box: the
/// palette's place.
const W: f32 = 520.0;
const TOP_GAP: f32 = 56.0;
/// The label column, and the gap after it.
const LABEL_W: f32 = 72.0;
const GAP: f32 = 10.0;
const PAD: f32 = 14.0;
const FIELD_H: f32 = 30.0;
const ROW_H: f32 = 26.0;
/// The preview scrolls past this many rows.
const ROWS_SHOWN: f32 = 8.0;
const PILL_FONT: f32 = 11.0;

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

/// Where the template field's caret goes on its next frame.
#[derive(Clone, Copy, Debug)]
enum Caret {
    /// Everything selected, so typing replaces it: how the box opens.
    All,
    At(usize),
}

/// The open box.
pub(crate) struct BatchRename {
    pub(crate) scope: Scope,
    /// The selection at open. Nothing can change it while the box is up.
    selection: Vec<NodeId>,
    pub(crate) find: String,
    /// *Rename to*, with its tokens spelled `[Name]`, `[Number]`, `[Group k]`.
    pub(crate) template: String,
    pub(crate) match_case: bool,
    pub(crate) pattern: bool,
    pub(crate) keep_case: bool,
    pub(crate) numbering: Numbering,
    /// From and Step as typed; the last whole number each held is in `numbering`.
    from_text: String,
    step_text: String,
    pub(crate) order: Order,
    /// *Include layers inside*, at Selection.
    pub(crate) inside: bool,
    /// *Include layers inside instances*, at page and document scope.
    pub(crate) inside_instances: bool,
    order_open: bool,
    /// The template's caret, in characters, while the field had it: where an
    /// insert button puts its token.
    caret: Option<usize>,
    focus: Option<Caret>,
}

impl BatchRename {
    fn set_scope(&mut self, scope: Scope) {
        self.scope = scope;
        // Away from the selection, the case of what matches is out of view.
        self.keep_case = scope != Scope::Selection;
    }
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

/// What the box would do with its current settings.
#[derive(Clone, Debug, Default)]
pub(crate) struct Plan {
    pub(crate) rows: Vec<Row>,
    /// The layers searched, past those left out inside instances.
    pub(crate) searched: usize,
    /// The layer rows listed.
    pub(crate) listed: usize,
    /// The pages with a listed row.
    pub(crate) pages: usize,
    /// Each layer that changes, and its new name, in the run's order.
    pub(crate) changes: Vec<(NodeId, String)>,
    /// Matches inside instances the box leaves out.
    pub(crate) skipped: usize,
    /// The pattern's groups, which the template can insert.
    pub(crate) groups: usize,
    /// Why Find renames nothing: a pattern that does not parse.
    pub(crate) error: Option<String>,
}

/// The layers under `page` in the layers tree's order, the page itself left out.
fn tree_order(doc: &Document, page: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![page];
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if id != page {
            out.push(id);
        }
        // Pushed in child order so the last child, the tree's top row, pops first.
        stack.extend(node.children().iter().copied());
    }
    out
}

/// Whether `id` sits inside an instance — not one that is an instance's root.
fn in_instance(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .and_then(|n| n.parent())
        .is_some_and(|p| ondin_core::component::instance_root(doc, p).is_some())
}

/// `ids` in reading order: rows (or columns, `!rows`) gathered by bands — a layer
/// joins the first band its centre falls in — then each band read along. Layers
/// with no bounds keep their order at the end.
fn by_bands(ids: Vec<NodeId>, res: &Resolved, rows: bool) -> Vec<NodeId> {
    let (mut boxed, mut rest) = (Vec::new(), Vec::new());
    for id in ids {
        match res.world_bounds(id) {
            Some(r) if rows => boxed.push((id, (r.y0, r.y1), r.x0)),
            Some(r) => boxed.push((id, (r.x0, r.x1), r.y0)),
            None => rest.push(id),
        }
    }
    /// A row or column: the span of the layer that started it, and its layers
    /// with where each starts along it.
    struct Band {
        lo: f64,
        hi: f64,
        members: Vec<(NodeId, f64)>,
    }
    boxed.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
    let mut bands: Vec<Band> = Vec::new();
    for (id, (lo, hi), along) in boxed {
        let centre = (lo + hi) / 2.0;
        match bands.iter_mut().find(|b| b.lo <= centre && centre <= b.hi) {
            Some(band) => band.members.push((id, along)),
            None => bands.push(Band {
                lo,
                hi,
                members: vec![(id, along)],
            }),
        }
    }
    let mut out: Vec<NodeId> = Vec::new();
    for mut band in bands {
        band.members.sort_by(|a, b| a.1.total_cmp(&b.1));
        out.extend(band.members.into_iter().map(|m| m.0));
    }
    out.extend(rest);
    out
}

/// What the box would rename from `page` with `b`'s settings: the layers in scope,
/// each page's in the run's order and the pages in the Pages list's, numbered
/// as they change.
pub(crate) fn plan(doc: &Document, res: &Resolved, page: NodeId, b: &BatchRename) -> Plan {
    let mut plan = Plan::default();
    let find = Find::new(&b.find, b.match_case, b.pattern);
    let rule = match find {
        Ok(find) => {
            plan.groups = find.groups();
            Some(Rule {
                find,
                template: Template::parse(&b.template),
                keep_case: b.keep_case,
            })
        }
        Err(e) => {
            plan.error = Some(e);
            None
        }
    };

    // Each page's layers, in tree order; the selection keeps only its own.
    let pages: Vec<NodeId> = match b.scope {
        Scope::Page => vec![page],
        Scope::Selection | Scope::Document => doc.page_ids().collect(),
    };
    let chosen: Option<HashSet<NodeId>> = (b.scope == Scope::Selection).then(|| {
        let mut set: HashSet<NodeId> = b.selection.iter().copied().collect();
        if b.inside {
            for id in &b.selection {
                set.extend(tree_order(doc, *id));
            }
        }
        set
    });
    let mut runs: Vec<(NodeId, Vec<NodeId>)> = Vec::new();
    for p in pages {
        let mut ids = Vec::new();
        for id in tree_order(doc, p) {
            match &chosen {
                Some(set) if !set.contains(&id) => {}
                Some(_) => ids.push(id),
                None if !b.inside_instances && in_instance(doc, id) => {
                    let name = doc.get(id).map_or("", |n| n.name());
                    if rule.as_ref().is_some_and(|r| r.apply(name, "").is_some()) {
                        plan.skipped += 1;
                    }
                }
                None => ids.push(id),
            }
        }
        let ids = match b.order {
            Order::Layers => ids,
            Order::Reversed => ids.into_iter().rev().collect(),
            Order::LeftToRight => by_bands(ids, res, true),
            Order::TopToBottom => by_bands(ids, res, false),
        };
        if !ids.is_empty() {
            runs.push((p, ids));
        }
    }

    // Only layers that change take a number, so a skipped one leaves no gap.
    let mut taken = 0usize;
    let every = b.scope == Scope::Selection;
    let many_pages = b.scope == Scope::Document;
    for (p, ids) in runs {
        plan.searched += ids.len();
        let mut rows = Vec::new();
        for id in ids {
            let Some(node) = doc.get(id) else { continue };
            let old = node.name().to_string();
            let applied = rule
                .as_ref()
                .and_then(|r| r.apply(&old, &b.numbering.nth(taken)));
            let (matched, new) = match applied {
                None => (Vec::new(), New::Unchanged),
                Some(r) if !node.variant().is_empty() => (r.matched, New::Variant),
                Some(r) => match ondin_core::naming::usable(&r.text()) {
                    None => (r.matched, New::Empty),
                    Some(name) if name == old => (r.matched, New::Unchanged),
                    Some(name) => {
                        taken += 1;
                        plan.changes.push((id, name));
                        (r.matched, New::Name(r.spans))
                    }
                },
            };
            if every || !matched.is_empty() {
                let (glyph, tint) = crate::panels::layers::node_icon(node.kind());
                rows.push(Row::Layer(LayerRow {
                    id,
                    glyph,
                    tint,
                    old,
                    matched,
                    new,
                }));
            }
        }
        if rows.is_empty() {
            continue;
        }
        plan.pages += 1;
        plan.listed += rows.len();
        if many_pages {
            plan.rows.push(Row::Page {
                name: doc
                    .get(p)
                    .map_or_else(String::new, |n| n.name().to_string()),
                count: rows.len(),
            });
        }
        plan.rows.extend(rows);
    }
    plan
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
/// padding so the caret still steps through real characters.
fn template_galley(ui: &egui::Ui, text: &str) -> std::sync::Arc<egui::Galley> {
    let font = egui::FontId::proportional(12.0);
    let plain = egui::TextFormat::simple(font.clone(), color::TEXT);
    let mut job = egui::text::LayoutJob::default();
    let mut at = 0;
    for (r, _) in ondin_core::rename::tokens(text) {
        job.append(&text[at..r.start], 0.0, plain.clone());
        let pill = |ink| egui::TextFormat {
            font_id: egui::FontId::proportional(PILL_FONT),
            color: ink,
            background: pill_fill(),
            valign: egui::Align::Center,
            ..Default::default()
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
    pub(crate) fn open_batch_rename(&mut self, search: Option<String>) {
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
        let numbering = Numbering {
            from: remembered.from,
            step: remembered.step,
            digits: remembered.digits.clamp(1, 3),
        };
        // One layer with children: renaming the layer alone is Rename's job.
        let inside = matches!(selection.as_slice(), [id] if doc.get(*id).is_some_and(|n| !n.children().is_empty()));
        let mut b = BatchRename {
            scope: Scope::Selection,
            selection,
            find: remembered.find,
            template: if remembered.template.is_empty() {
                "[Name]".to_string()
            } else {
                remembered.template
            },
            match_case: false,
            pattern: false,
            keep_case: false,
            numbering,
            from_text: numbering.from.to_string(),
            step_text: numbering.step.to_string(),
            order: Order::from_key(&remembered.order),
            inside,
            inside_instances: false,
            order_open: false,
            caret: None,
            focus: Some(Caret::All),
        };
        let search = search.filter(|s| !s.trim().is_empty());
        if b.selection.is_empty() || search.is_some() {
            b.set_scope(Scope::Page);
        }
        if let Some(s) = search {
            b.find = s.trim().to_string();
        }
        self.batch_rename = Some(b);
    }

    /// Rename every layer the plan changes, as one undo step, and remember the
    /// box's Find, template and numbering for next time. Called only when the plan
    /// changes something.
    fn commit_batch_rename(&mut self, b: &BatchRename, plan: &Plan) {
        let br = &mut self.prefs.batch_rename;
        br.find = b.find.clone();
        br.template = b.template.clone();
        br.from = b.numbering.from;
        br.step = b.numbering.step;
        br.digits = b.numbering.digits;
        br.order = b.order.key().to_string();
        self.prefs.save();
        let tx = Transaction(
            plan.changes
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
                layers(plan.changes.len())
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
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if escape && b.order_open {
            b.order_open = false;
            self.batch_rename = Some(b);
            return;
        }
        if escape {
            return;
        }
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let plan = plan(
            &self.session.doc,
            &self.session.resolved,
            self.session.page,
            &b,
        );

        let screen = ctx.content_rect();
        let id = egui::Id::new("batch-rename");
        let area = egui::Modal::default_area(id).anchor(
            egui::Align2::CENTER_TOP,
            egui::vec2(
                canvas.center().x - screen.center().x,
                canvas.top() - screen.top() + TOP_GAP,
            ),
        );
        let mut act = Act::None;
        let mut order_button = egui::Rect::NOTHING;
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
                        scope_row(ui, &mut b);
                        find_row(ui, &mut b, plan.error.as_deref());
                        template_row(ui, &mut b, plan.groups);
                        if Template::parse(&b.template).has_number() {
                            order_button = number_row(ui, &mut b);
                        }
                        checks(ui, &mut b, plan.skipped);
                    });
                ui.add_space(8.0);
                divider(ui);
                preview(ui, b.scope, &plan);
                divider(ui);
                if let Some(a) = foot(ui, plan.changes.len()) {
                    act = a;
                }
            });
        // The Number row goes when its token does, and its menu with it.
        b.order_open &= order_button != egui::Rect::NOTHING;
        if b.order_open
            && let Some(o) = order_menu(ctx, order_button, b.order)
        {
            b.order = o;
            b.order_open = false;
        }
        // A click outside closes the order menu first, then the box.
        if modal.backdrop_response.clicked() {
            if !b.order_open {
                return;
            }
            b.order_open = false;
        }
        if enter && !b.order_open {
            act = Act::Rename;
        }
        match act {
            Act::Cancel => {}
            Act::Rename if !plan.changes.is_empty() => {
                self.commit_batch_rename(&b, &plan);
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
    crate::ui::icon_button(&mut child, icon::X, 26.0, 13.0, false, true)
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

/// A segment of a segmented control, or a toggle in a field: lit when `on`.
fn segment(
    ui: &mut egui::Ui,
    text: &str,
    sub: &str,
    on: bool,
    h: f32,
    mono: bool,
) -> egui::Response {
    let font = match mono {
        true => egui::FontId::monospace(11.0),
        false => egui::FontId::proportional(11.5),
    };
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), font.clone(), color::TEXT));
    let sub_g = (!sub.is_empty()).then(|| {
        ui.fonts_mut(|f| {
            f.layout_no_wrap(
                sub.to_string(),
                egui::FontId::proportional(11.5),
                theme::text::DIM,
            )
        })
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

/// A rounded well holding segments.
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

fn scope_row(ui: &mut egui::Ui, b: &mut BatchRename) {
    row(ui, "In", ROW_H, |ui| {
        well(ui, |ui| {
            let n = match b.selection.len() {
                0 => String::new(),
                n => n.to_string(),
            };
            for (scope, label, sub) in [
                (Scope::Selection, "Selection", n.as_str()),
                (Scope::Page, "This page", ""),
                (Scope::Document, "All pages", ""),
            ] {
                let live = scope != Scope::Selection || !b.selection.is_empty();
                let resp = ui.add_enabled_ui(live, |ui| {
                    segment(ui, label, sub, b.scope == scope, 22.0, false)
                });
                let resp = resp.inner;
                let resp = if live {
                    resp
                } else {
                    resp.on_disabled_hover_text("Nothing is selected")
                };
                if resp.clicked() && b.scope != scope {
                    b.set_scope(scope);
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

/// A field's well and ring, lit while `focused`, laid out `w` wide.
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

fn find_row(ui: &mut egui::Ui, b: &mut BatchRename, error: Option<&str>) {
    row(ui, "Find", FIELD_H, |ui| {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let w = ui.available_width();
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, FIELD_H), egui::Sense::hover());
            let id = egui::Id::new(FIND_ID);
            let focused = ui.memory(|m| m.has_focus(id));
            field_ground(ui, rect, focused);
            let text = egui::Rect::from_min_max(
                rect.min + egui::vec2(9.0, 0.0),
                egui::pos2(rect.right() - 4.0 - 52.0, rect.bottom()),
            );
            ui.put(
                text,
                egui::TextEdit::singleline(&mut b.find)
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
            let toggles = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 4.0 - 52.0, rect.top()),
                egui::pos2(rect.right() - 4.0, rect.bottom()),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(toggles)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            child.spacing_mut().item_spacing.x = 4.0;
            field_toggle(&mut child, "Aa", "Match case", &mut b.match_case, false);
            field_toggle(&mut child, ".*", "Find as a pattern", &mut b.pattern, true);
            if let Some(e) = error {
                ui.label(egui::RichText::new(e).size(10.5).color(color::WARN));
            }
        });
    });
}

/// Rename to: the template field with its tokens as pills, and the buttons that
/// insert them.
fn template_row(ui: &mut egui::Ui, b: &mut BatchRename, groups: usize) {
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
                take_pill(ui, id, &mut b.template);
            }
            let text = egui::Rect::from_min_max(
                rect.min + egui::vec2(9.0, 0.0),
                egui::pos2(rect.right() - 4.0 - 24.0, rect.bottom()),
            );
            let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, _wrap: f32| {
                template_galley(ui, buf.as_str())
            };
            let out = egui::TextEdit::singleline(&mut b.template)
                .id(id)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center)
                .desired_width(text.width())
                .font(egui::FontId::proportional(12.0))
                .layouter(&mut layouter)
                .show(&mut ui.new_child(egui::UiBuilder::new().max_rect(text)));
            let caret = out.cursor_range.map(|c| c.primary.index.0);
            if out.response.changed() {
                let (text, moved) = canonical(&b.template, caret.unwrap_or(0));
                if text != b.template {
                    b.template = text;
                    b.focus = Some(Caret::At(moved));
                }
            }
            if out.response.has_focus() {
                b.caret = caret;
                // A caret inside a pill steps out of it, to its far side.
                if let Some(c) = out.cursor_range
                    && c.primary.index == c.secondary.index
                    && let Some(r) = token_at(&b.template, c.primary.index.0, false)
                    && r.start < c.primary.index.0
                {
                    b.focus = Some(Caret::At(r.end));
                }
            }
            if let Some(f) = b.focus.take() {
                let len = b.template.chars().count();
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

            let toggle = egui::Rect::from_min_max(
                egui::pos2(rect.right() - 4.0 - 24.0, rect.top()),
                egui::pos2(rect.right() - 4.0, rect.bottom()),
            );
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(toggle)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            field_toggle(
                &mut child,
                "Ab",
                "Keep each match’s case",
                &mut b.keep_case,
                false,
            );

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let mut insert = |ui: &mut egui::Ui, label: &str, piece: Piece| {
                    if insert_button(ui, label).clicked() {
                        let token = Template(vec![piece]).to_string();
                        let len = b.template.chars().count();
                        let at = b.caret.unwrap_or(len).min(len);
                        let byte = char_to_byte(&b.template, at);
                        b.template.insert_str(byte, &token);
                        b.focus = Some(Caret::At(at + token.chars().count()));
                    }
                };
                insert(ui, "Name", Piece::Name);
                insert(ui, "Number", Piece::Number);
                for k in 1..=groups.min(9) {
                    insert(ui, &format!("Group {k}"), Piece::Group(k));
                }
                if b.template.is_empty() && !b.find.is_empty() {
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

/// A whole-number field; `value` takes what it holds whenever that parses.
fn int_field(ui: &mut egui::Ui, id: &str, text: &mut String, value: &mut i64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(34.0, ROW_H), egui::Sense::hover());
    let id = egui::Id::new(id);
    let focused = ui.memory(|m| m.has_focus(id));
    field_ground(ui, rect, focused);
    let resp = ui.put(
        rect.shrink2(egui::vec2(7.0, 0.0)),
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::ZERO)
            .vertical_align(egui::Align::Center)
            .font(egui::FontId::proportional(12.0))
            .text_color(color::TEXT)
            .char_limit(6),
    );
    if resp.changed()
        && let Ok(n) = text.trim().parse::<i64>()
    {
        *value = n;
    }
    // Leaving puts back the last whole number; nothing commits until Rename.
    // valve-gate-not-a-commit
    if resp.lost_focus() {
        *text = value.to_string();
    }
}

/// The numbering row; returns the order button's rect, which its menu hangs from.
fn number_row(ui: &mut egui::Ui, b: &mut BatchRename) -> egui::Rect {
    row(ui, "Number", ROW_H, |ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let small = |ui: &mut egui::Ui, t: &str| {
            ui.label(egui::RichText::new(t).size(11.0).color(theme::text::DIM));
        };
        small(ui, "From");
        int_field(
            ui,
            "batch-rename-from",
            &mut b.from_text,
            &mut b.numbering.from,
        );
        ui.add_space(6.0);
        small(ui, "Step");
        int_field(
            ui,
            "batch-rename-step",
            &mut b.step_text,
            &mut b.numbering.step,
        );
        ui.add_space(6.0);
        well(ui, |ui| {
            for (digits, label) in [(1u8, "1"), (2, "01"), (3, "001")] {
                if segment(ui, label, "", b.numbering.digits == digits, 22.0, false)
                    .on_hover_text(format!(
                        "Pad to {digits} digit{}",
                        if digits == 1 { "" } else { "s" }
                    ))
                    .clicked()
                {
                    b.numbering.digits = digits;
                }
            }
        });
        ui.add_space(6.0);
        let font = egui::FontId::proportional(12.0);
        let galley =
            ui.fonts_mut(|f| f.layout_no_wrap(b.order.label().to_string(), font, color::TEXT));
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(8.0 + 13.0 + 6.0 + galley.size().x + 6.0 + 10.0 + 6.0, ROW_H),
            egui::Sense::click(),
        );
        let p = ui.painter();
        p.rect_filled(
            rect,
            5.0,
            if resp.hovered() {
                color::HOVER
            } else {
                color::FIELD
            },
        );
        let y = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 8.0, y),
            egui::Align2::LEFT_CENTER,
            b.order.glyph(),
            theme::icon_font(13.0),
            theme::text::DIM,
        );
        p.galley(
            egui::pos2(rect.left() + 27.0, y - galley.size().y / 2.0),
            galley,
            color::TEXT,
        );
        p.text(
            egui::pos2(rect.right() - 6.0, y),
            egui::Align2::RIGHT_CENTER,
            icon::CARET_DOWN,
            theme::icon_font(10.0),
            theme::text::DIM,
        );
        if resp.on_hover_text("Which layer is first").clicked() {
            b.order_open = !b.order_open;
        }
        rect
    })
}

/// The order menu under its button; the order picked, if one was.
fn order_menu(ctx: &egui::Context, under: egui::Rect, now: Order) -> Option<Order> {
    let size = egui::vec2(220.0, 4.0 * 28.0 + 8.0);
    let at = crate::ui::dropdown_place(ctx, under, size, 4.0);
    let mut picked = None;
    egui::Area::new(egui::Id::new("batch-rename-order"))
        .order(egui::Order::Tooltip)
        .fixed_pos(at)
        .show(ctx, |ui| {
            crate::ui::menu_frame(4.0).show(ui, |ui| {
                ui.set_width(size.x - 8.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for o in Order::ALL {
                    let row = crate::ui::MenuRow::new(o.glyph(), o.label()).checked(o == now);
                    if crate::ui::menu_row(ui, row, 28.0).clicked() {
                        picked = Some(o);
                    }
                }
            });
        });
    picked
}

fn checkbox(ui: &mut egui::Ui, label: &str, note: &str, on: &mut bool) {
    ui.horizontal(|ui| {
        ui.add_space(LABEL_W + GAP);
        ui.spacing_mut().item_spacing.x = 7.0;
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

fn checks(ui: &mut egui::Ui, b: &mut BatchRename, skipped: usize) {
    match b.scope {
        Scope::Selection => checkbox(ui, "Include layers inside", "", &mut b.inside),
        Scope::Page | Scope::Document => {
            let note = match skipped {
                0 => String::new(),
                n => format!("· {} skipped", count(n)),
            };
            checkbox(
                ui,
                "Include layers inside instances",
                &note,
                &mut b.inside_instances,
            );
        }
    }
}

/// The preview: every layer at Selection, the matches elsewhere.
fn preview(ui: &mut egui::Ui, scope: Scope, plan: &Plan) {
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
            egui::ScrollArea::vertical()
                .id_salt("batch-rename-preview")
                .max_height(ROW_H * ROWS_SHOWN)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if plan.rows.is_empty() {
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
                        ui.painter().text(
                            rect.left_center() + egui::vec2(8.0, 0.0),
                            egui::Align2::LEFT_CENTER,
                            "No layer matches",
                            egui::FontId::proportional(12.0),
                            theme::text::DIM,
                        );
                    }
                    for r in &plan.rows {
                        match r {
                            Row::Page { name, count } => page_head(ui, name, *count),
                            Row::Layer(l) => layer_row(ui, l),
                        }
                    }
                });
        });
}

fn page_head(ui: &mut egui::Ui, name: &str, n: usize) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W - 12.0, 24.0), egui::Sense::hover());
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

/// The footer's buttons; what was clicked.
fn foot(ui: &mut egui::Ui, changes: usize) -> Option<Act> {
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
            .max_rect(rect.shrink2(egui::vec2(10.0, 10.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    let primary = button(&mut child, &go, true, changes > 0);
    let cancel = button(&mut child, "Cancel", false, true);
    if primary.clicked() && changes > 0 {
        Some(Act::Rename)
    } else if cancel.clicked() {
        Some(Act::Cancel)
    } else {
        None
    }
}

fn button(ui: &mut egui::Ui, label: &str, primary: bool, live: bool) -> egui::Response {
    let font = egui::FontId::proportional(12.0);
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font, color::TEXT));
    let sense = if live {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(galley.size().x + 24.0, 28.0), sense);
    let p = ui.painter();
    let (fill, ink) = match (primary, live) {
        (true, true) if resp.hovered() => (color::ACCENT_400, egui::Color32::WHITE),
        (true, true) => (color::ACCENT, egui::Color32::WHITE),
        (true, false) => (color::ACCENT_800, color::text_a(110)),
        (false, _) if resp.hovered() => (color::text_a(30), color::TEXT),
        (false, _) => (color::text_a(20), theme::text::STRONG),
    };
    p.rect_filled(rect, 5.0, fill);
    p.galley(rect.center() - galley.size() / 2.0, galley, ink);
    resp
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
    fn opened(app: &mut OndinApp, find: &str, template: &str) -> BatchRename {
        app.open_batch_rename(None);
        let mut b = app.batch_rename.take().expect("the box opened");
        b.find = find.into();
        b.template = template.into();
        b
    }

    fn planned(app: &OndinApp, b: &BatchRename) -> Plan {
        plan(&app.session.doc, &app.session.resolved, app.session.page, b)
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
    /// flip: `plan` counting `taken` for every listed layer → fails at the second
    /// name, `Item 3 a`.
    #[test]
    fn a_run_is_numbered_in_tree_order_and_a_skip_leaves_no_gap() {
        let (_ctx, mut app) = app();
        let a = named(&mut app, "Rect a");
        let skip = named(&mut app, "Other");
        let b = named(&mut app, "Rect b");
        app.session.selection.set(vec![a, skip, b]);
        let bx = opened(&mut app, "Rect", "Item [Number]");
        assert_eq!(bx.scope, Scope::Selection);
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
        assert_eq!(count_label(bx.scope, &plan), "2 of 3 change");
    }

    /// *Left to right* takes rows by band, though no two tops line up, then reads
    /// each row left to right; *Top to bottom* takes columns the same way.
    ///
    /// flip: `by_bands` putting every layer in one band (sorting by the left edge
    /// alone) → fails at the left-to-right order. Joining a band by the top rather
    /// than the centre gives the same answer here.
    #[test]
    fn the_canvas_orders_read_by_band() {
        let (_ctx, mut app) = app();
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
        let mut bx = opened(&mut app, "", "[Name][Number]");
        bx.order = Order::LeftToRight;
        let got: Vec<String> = planned(&app, &bx)
            .changes
            .into_iter()
            .map(|c| c.1)
            .collect();
        assert_eq!(got, ["a1", "b2", "c3", "d4", "e5", "f6"]);
        bx.order = Order::TopToBottom;
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
        let (_ctx, mut app) = app();
        let hero = named(&mut app, "Draft Hero");
        let bare = named(&mut app, "Draft");
        app.session.selection.set(vec![hero, bare]);
        let bx = opened(&mut app, "Draft", "");
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

    /// A pattern that does not parse renames nothing and says why.
    #[test]
    fn a_bad_pattern_renames_nothing() {
        let (_ctx, mut app) = app();
        let a = named(&mut app, "Card");
        app.session.selection.set_one(a);
        let mut bx = opened(&mut app, "(", "x");
        bx.pattern = true;
        let plan = planned(&app, &bx);
        assert!(plan.changes.is_empty());
        assert!(
            plan.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Not a pattern"))
        );
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
    /// flip: `plan` ignoring `inside_instances` → fails at "the copies are left out".
    #[test]
    fn the_page_leaves_instance_insides_out() {
        let (_ctx, mut app, _) = with_an_instance();
        let mut bx = opened(&mut app, "button", "cta");
        assert_eq!(bx.scope, Scope::Page, "nothing selected opens on the page");
        assert!(bx.keep_case, "keep case is on away from the selection");
        let plan = planned(&app, &bx);
        assert_eq!(plan.listed, 2, "the copies are left out");
        assert_eq!(plan.skipped, 2);
        assert!(
            plan.changes.iter().all(|c| c.1.starts_with("Cta ")),
            "{:?}",
            plan.changes
        );
        bx.inside_instances = true;
        let plan = planned(&app, &bx);
        assert_eq!((plan.listed, plan.skipped), (4, 0));
    }

    /// All pages heads each page's rows with its name and count, in the Pages
    /// list's order, and the number runs on across pages.
    #[test]
    fn all_pages_heads_each_page_and_numbers_on() {
        let (_ctx, mut app) = app();
        named(&mut app, "Hero");
        named(&mut app, "Footer");
        let first = app.session.page;
        app.add_page();
        named(&mut app, "hero image");
        app.go_to_page(first);
        let mut bx = opened(&mut app, "hero", "[Name] [Number]");
        bx.set_scope(Scope::Document);
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
        let (_ctx, mut app, m) = with_an_instance();
        app.session.selection.set_one(m);
        app.open_batch_rename(None);
        let b = app.batch_rename.take().unwrap();
        assert_eq!(
            (b.scope, b.inside, b.keep_case),
            (Scope::Selection, true, false)
        );
        app.open_batch_rename(Some(" button ".into()));
        let b = app.batch_rename.take().unwrap();
        assert_eq!((b.scope, b.find.as_str()), (Scope::Page, "button"));
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
    ) {
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
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
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
            app.batch_rename.as_ref().map(|b| b.template.as_str()),
            Some("Card [Number]"),
            "typing replaced the Name pill"
        );
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
        app.open_batch_rename(None);
        pass(&ctx, &mut app, Vec::new(), none);
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text("x[number]".into())],
            none,
        );
        pass(&ctx, &mut app, Vec::new(), none);
        assert_eq!(app.batch_rename.as_ref().unwrap().template, "x[Number]");
        pass(&ctx, &mut app, vec![key(egui::Key::Backspace, none)], none);
        assert_eq!(
            app.batch_rename.as_ref().unwrap().template,
            "x",
            "the pill went whole"
        );
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, none)], none);
        assert!(app.batch_rename.is_none());
        assert_eq!(app.session.doc.get(a).unwrap().name(), "Rectangle 1");
    }
}
