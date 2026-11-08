//! *Clean up…* (`docs/design/app-canvas-chrome.md`, *Clean up*): the box over the
//! canvas that takes away what a design no longer shows, in the selection, the
//! page or every page.
//!
//! The rule is `ondin_core::clean_up`'s; this module draws batch rename's box with
//! a switch per rule, a preview grouped by rule, and commits the plan as one undo
//! step. The document cannot change while the box is up, so a plan is made only
//! when the scope or a switch changes.

use crate::app::OndinApp;
use crate::batch_rename::{Scope, row, scope_segments};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::clean_up::{self, Finding, Plan, Rule, Rules, Subject};
use ondin_core::{Document, GuideAxis, NodeId, Resolved};
use std::collections::HashSet;

/// The box's width, batch rename's.
const W: f32 = 520.0;
const GAP: f32 = 10.0;
const PAD: f32 = crate::settings::CANVAS_CARD_PAD;
/// The air between the two columns of switches.
const COL_GAP: f32 = 16.0;
/// A preview row: a line of a list, a menu row's height.
const ROW_H: f32 = ui::MENU_ROW_H;
/// The preview scrolls past this many rows.
const ROWS_SHOWN: f32 = 12.0;
/// A group lists this many rows before its *N more* row.
const FOLDED: usize = 3;
/// The switches two to a line, paint rules on the left and layer rules on the right.
const GRID: [[Rule; 2]; 3] = [
    [Rule::HiddenPaint, Rule::HiddenLayers],
    [Rule::CoveredFills, Rule::EmptyLayers],
    [Rule::OneLayerGroups, Rule::Guides],
];

/// The open box.
pub(crate) struct CleanUpBox {
    pub(crate) scope: Scope,
    /// The selection at open, which *Selection* means.
    selection: Vec<NodeId>,
    pub(crate) rules: Rules,
    /// The plan for the scope and switches shown.
    pub(crate) plan: Plan,
    /// The plan with every switch on, for the counts beside the switches.
    every: Plan,
    /// What `plan` and `every` were made for.
    asked: Option<(Scope, Rules)>,
    /// Groups opened past their first rows.
    open: HashSet<Rule>,
    /// What the hovered row names, and whether it is a whole group's (a hairline).
    hover: (Vec<NodeId>, bool),
}

/// One line of the preview.
enum Line {
    Head(Rule, usize),
    Row(usize),
    More(Rule, usize),
}

impl CleanUpBox {
    /// The layers the canvas rings for the hovered row, and whether softly.
    pub(crate) fn ringed(&self) -> (&[NodeId], bool) {
        (&self.hover.0, self.hover.1)
    }

    fn core_scope(&self, doc: &Document, page: NodeId) -> clean_up::Scope {
        match self.scope {
            Scope::Selection => clean_up::Scope::Layers(self.selection.clone()),
            Scope::Page => clean_up::Scope::Pages(vec![page]),
            // Drafts are left out, as everything gathered from every page is, but
            // the current page is kept: the box was opened from it.
            Scope::Document => clean_up::Scope::Pages(
                doc.pages()
                    .iter()
                    .filter(|p| !p.draft || p.id == page)
                    .map(|p| p.id)
                    .collect(),
            ),
        }
    }

    /// Plan again when the scope or a switch has changed since the last plan.
    pub(crate) fn replan(&mut self, doc: &Document, res: &Resolved, page: NodeId) {
        let now = (self.scope, self.rules);
        if self.asked == Some(now) {
            return;
        }
        let scope = self.core_scope(doc, page);
        self.plan = clean_up::plan(doc, res, &scope, &self.rules);
        self.every = clean_up::plan(doc, res, &scope, &self.rules.all_on());
        self.asked = Some(now);
    }

    /// The preview's lines: each rule's heading, its first rows, and a *N more*
    /// row unless opened.
    fn lines(&self) -> Vec<Line> {
        let mut out = Vec::new();
        for rule in Rule::ALL {
            let rows: Vec<usize> = (0..self.plan.findings.len())
                .filter(|i| self.plan.findings[*i].rule == rule)
                .collect();
            if rows.is_empty() {
                continue;
            }
            out.push(Line::Head(rule, self.plan.items_of(rule)));
            let shown = if self.open.contains(&rule) {
                rows.len()
            } else {
                rows.len().min(FOLDED)
            };
            out.extend(rows[..shown].iter().map(|i| Line::Row(*i)));
            if shown < rows.len() {
                out.push(Line::More(rule, rows.len() - shown));
            }
        }
        out
    }

    /// Why *Clean up* is dim, or `None` when it would take something.
    fn why_not(&self) -> Option<&'static str> {
        (self.plan.items() == 0).then_some("Nothing to clean up")
    }
}

/// The preview's count: *N items on M layers*, and *· K kept* when amber rows show.
fn count_label(plan: &Plan) -> String {
    let mut out = format!(
        "{} on {}",
        ui::count(plan.items(), "item"),
        ui::count(plan.layers(), "layer")
    );
    if plan.kept() > 0 {
        out.push_str(&format!(" · {} kept", plan.kept()));
    }
    out
}

/// What the empty preview says for `scope`.
fn nothing_here(scope: Scope) -> &'static str {
    match scope {
        Scope::Selection => "Nothing to clean up in the selection",
        Scope::Page => "Nothing to clean up on this page",
        Scope::Document => "Nothing to clean up on any page",
    }
}

/// The switches that are off but would take something: *Hidden layers 3 · Guides 2*.
fn turned_off(b: &CleanUpBox) -> Option<String> {
    let off: Vec<String> = Rule::ALL
        .into_iter()
        .filter(|r| !b.rules.is_on(*r) && b.every.items_of(*r) > 0)
        .map(|r| format!("{} {}", r.label(), b.every.items_of(r)))
        .collect();
    (!off.is_empty()).then(|| format!("Turned off: {}", off.join(" · ")))
}

impl OndinApp {
    /// *Clean up…*: open the box on the selection when there is one, else on the
    /// page, with the switches last used.
    pub(crate) fn open_clean_up(&mut self) {
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        self.picker = None;
        // The path bar's card is the same act on one path's points; the two
        // never show together.
        self.cleanup = None;
        let doc = &self.session.doc;
        let selection: Vec<NodeId> = self
            .session
            .selection
            .ids()
            .iter()
            .copied()
            .filter(|id| !doc.is_page(*id) && doc.get(*id).is_some())
            .collect();
        let remembered = &self.prefs.clean_up;
        let mut rules = Rules {
            skip_locked: remembered.skip_locked,
            guides_locked: self.lock_guides,
            ..Rules::default()
        };
        for rule in Rule::ALL {
            rules.set(rule, remembered.on.iter().any(|k| k == rule.key()));
        }
        self.clean_up = Some(CleanUpBox {
            scope: match selection.is_empty() {
                true => Scope::Page,
                false => Scope::Selection,
            },
            selection,
            rules,
            plan: Plan::default(),
            every: Plan::default(),
            asked: None,
            open: HashSet::new(),
            hover: (Vec::new(), false),
        });
    }

    /// Run the plan as one undo step and remember the switches.
    fn commit_clean_up(&mut self, b: &CleanUpBox) {
        let p = &mut self.prefs.clean_up;
        p.on = Rule::ALL
            .into_iter()
            .filter(|r| b.rules.is_on(*r))
            .map(|r| r.key().to_string())
            .collect();
        p.skip_locked = b.rules.skip_locked;
        self.prefs.save();
        let items = b.plan.items();
        self.session.name_step("Clean up", icon::MAGIC_WAND);
        if self.session.commit(b.plan.tx.clone()) {
            let page = self.session.page;
            let doc = &self.session.doc;
            self.session.selection.retain_existing(doc, page);
            self.session.info(format!(
                "Cleaned up {} · Ctrl+Z to undo",
                ui::count(items, "item")
            ));
        }
    }

    /// Draw the open box and answer this frame's keys and clicks: a modal with a
    /// clear backdrop, as batch rename's, so the canvas stays live behind it.
    pub(crate) fn clean_up_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut b) = self.clean_up.take() else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        let enter = ui::card_enter(ctx);
        b.replan(&self.session.doc, &self.session.resolved, self.session.page);
        let why_not = b.why_not();
        let mut act = Act::None;
        let mut hover: (Vec<NodeId>, bool) = (Vec::new(), false);
        let doc = &self.session.doc;
        let page = self.session.page;
        let hold = crate::settings::CardHold::Modal;
        let clicked_away = crate::settings::canvas_card(ctx, "clean-up", canvas, W, hold, |ui| {
            if crate::settings::card_head(ui, W, "Clean up") {
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
                    switches(ui, &mut b);
                });
            ui.add_space(8.0);
            ui::card_rule(ui);
            preview(ui, doc, page, &mut b, &mut hover);
            ui::card_rule(ui);
            if let Some(a) = foot(ui, b.plan.items(), why_not) {
                act = a;
            }
        });
        b.hover = hover;
        if clicked_away {
            return;
        }
        if enter && matches!(act, Act::None) {
            act = Act::Run;
        }
        match act {
            Act::Cancel => {}
            Act::Run if why_not.is_none() => {
                self.commit_clean_up(&b);
                ctx.request_repaint();
            }
            _ => self.clean_up = Some(b),
        }
    }
}

enum Act {
    None,
    Run,
    Cancel,
}

fn scope_row(ui: &mut egui::Ui, b: &mut CleanUpBox) {
    const SCOPES: [Scope; 3] = [Scope::Selection, Scope::Page, Scope::Document];
    row(ui, "In", ui::CONTROL_H, |ui| {
        let lit = SCOPES.iter().position(|x| *x == b.scope).unwrap_or(1);
        if let Some(i) = scope_segments(ui, lit, b.selection.len(), ui::SEGMENT_CELL_H) {
            b.scope = SCOPES[i];
        }
    });
}

/// One switch, its count beside its label; the row's tooltip says what it takes.
fn switch(ui: &mut egui::Ui, label: &str, count: Option<usize>, on: &mut bool, hint: &str) {
    let note = count
        .filter(|n| *n > 0)
        .map(|n| n.to_string())
        .unwrap_or_default();
    if ui::switch_row_noted(ui, label, &note, *on, ui::CONTROL_H)
        .on_hover_text(hint)
        .clicked()
    {
        *on = !*on;
    }
}

/// *Remove*: the rules two to a line, then *Skip locked layers*.
fn switches(ui: &mut egui::Ui, b: &mut CleanUpBox) {
    let every = &b.every;
    let rules = &mut b.rules;
    row(ui, "Remove", ui::CONTROL_H * 3.0 + 4.0, |ui| {
        let col = (ui.available_width() - COL_GAP) / 2.0;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for pair in GRID {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = COL_GAP;
                    for rule in pair {
                        ui.allocate_ui(egui::vec2(col, ui::CONTROL_H), |ui| {
                            let mut on = rules.is_on(rule);
                            switch(
                                ui,
                                rule.label(),
                                Some(every.items_of(rule)),
                                &mut on,
                                rule.hint(),
                            );
                            rules.set(rule, on);
                        });
                    }
                });
            }
        });
    });
    row(ui, "", ui::CONTROL_H, |ui| {
        let col = (ui.available_width() - COL_GAP) / 2.0;
        ui.allocate_ui(egui::vec2(col, ui::CONTROL_H), |ui| {
            switch(
                ui,
                "Skip locked layers",
                None,
                &mut rules.skip_locked,
                "List locked layers and leave them as they are",
            );
        });
    });
}

/// The preview: each rule's rows under its heading. Hovering a row rings its layer
/// on the canvas; hovering a heading rings the group's, softly.
fn preview(
    ui: &mut egui::Ui,
    doc: &Document,
    page: NodeId,
    b: &mut CleanUpBox,
    hover: &mut (Vec<NodeId>, bool),
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 30.0), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.bottom() - 10.0;
    ui::paint_eyebrow(
        p,
        egui::pos2(rect.left() + PAD, y),
        egui::Align2::LEFT_CENTER,
        "Preview",
    );
    p.text(
        egui::pos2(rect.right() - PAD, y),
        egui::Align2::RIGHT_CENTER,
        count_label(&b.plan),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::dim(),
    );
    let lines = b.lines();
    let mut opened = None;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 6,
            right: 6,
            top: 0,
            bottom: 8,
        })
        .show(ui, |ui| {
            if lines.is_empty() {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for note in
                        std::iter::once(nothing_here(b.scope).to_string()).chain(turned_off(b))
                    {
                        let (rect, _) = ui
                            .allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
                        ui.scope_builder(
                            egui::UiBuilder::new()
                                .max_rect(rect.with_min_x(rect.left() + 8.0))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            |ui| ui::empty_note(ui, &note),
                        );
                    }
                });
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("clean-up-preview")
                .max_height(ROW_H * ROWS_SHOWN)
                .auto_shrink([false, true])
                .show_rows(ui, ROW_H, lines.len(), |ui, range| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for line in &lines[range] {
                        match line {
                            Line::Head(rule, n) => {
                                if head(ui, rule.label(), *n).hovered() {
                                    *hover = (group_layers(&b.plan, *rule, doc, page), true);
                                }
                            }
                            Line::Row(i) => {
                                let f = &b.plan.findings[*i];
                                if finding_row(ui, doc, page, f).hovered() {
                                    *hover = (ringable(f, doc, page).into_iter().collect(), false);
                                }
                            }
                            Line::More(rule, n) => {
                                if more_row(ui, *n).clicked() {
                                    opened = Some(*rule);
                                }
                            }
                        }
                    }
                });
        });
    if let Some(rule) = opened {
        b.open.insert(rule);
    }
}

/// The layer a row rings: one on this page, still there.
fn ringable(f: &Finding, doc: &Document, page: NodeId) -> Option<NodeId> {
    match f.subject {
        Subject::Layer(id) if doc.page_of(id) == Some(page) => Some(id),
        _ => None,
    }
}

fn group_layers(plan: &Plan, rule: Rule, doc: &Document, page: NodeId) -> Vec<NodeId> {
    plan.findings
        .iter()
        .filter(|f| f.rule == rule)
        .filter_map(|f| ringable(f, doc, page))
        .collect()
}

fn head(ui: &mut egui::Ui, label: &str, n: usize) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::dim(),
    );
    p.text(
        egui::pos2(rect.right() - 8.0, y),
        egui::Align2::RIGHT_CENTER,
        n.to_string(),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    resp
}

/// A layer's name with its parent's before it (*Card / Background*), masked in
/// private mode.
fn layer_name(doc: &Document, id: NodeId) -> String {
    let own = crate::private::layer(doc, id).into_owned();
    match doc.get(id).and_then(|n| n.parent()) {
        Some(parent) if !doc.is_page(parent) => {
            format!("{} / {own}", crate::private::layer(doc, parent))
        }
        _ => own,
    }
}

fn finding_row(ui: &mut egui::Ui, doc: &Document, page: NodeId, f: &Finding) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::hover());
    ui::list_row_ground(ui.painter(), rect, false, resp.hovered());
    let (glyph, tint, name) = match f.subject {
        Subject::Layer(id) => {
            let (glyph, tint) = doc
                .get(id)
                .map(|n| crate::panels::layers::node_glyph(n.kind()))
                .map_or((icon::SQUARE, theme::text::dim()), |(g, t)| (g, t.colour()));
            (glyph, tint, layer_name(doc, id))
        }
        Subject::Guide { axis, position, .. } => {
            let word = match axis {
                GuideAxis::Horizontal => "Horizontal guide",
                GuideAxis::Vertical => "Vertical guide",
            };
            (
                icon::RULER,
                theme::text::dim(),
                format!("{word} · {}", position.round()),
            )
        }
    };
    let p = ui.painter();
    let y = rect.center().y;
    let inner = rect.shrink2(egui::vec2(8.0, 0.0));
    p.text(
        egui::pos2(inner.left() + 8.0, y),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(14.0),
        tint,
    );
    let font = egui::FontId::proportional(ui::CONTROL_PT);
    let half = (inner.width() - 24.0) / 2.0;
    let mut job = egui::text::LayoutJob::single_section(
        name,
        egui::TextFormat::simple(font.clone(), theme::text::muted()),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(half);
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    p.galley(
        egui::pos2(inner.left() + 24.0, y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    let (what, ink) = match f.kept {
        Some(k) => (k.words(), color::warn()),
        None => (f.what.clone(), theme::text::dim()),
    };
    let mut job = egui::text::LayoutJob::single_section(what, egui::TextFormat::simple(font, ink));
    job.wrap = egui::text::TextWrapping::truncate_at_width(half);
    let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
    p.galley(
        egui::pos2(inner.right() - galley.size().x, y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    // A row on another page rings nothing; its tooltip says where it is.
    match f.subject {
        Subject::Layer(id) if doc.page_of(id).is_some_and(|p| p != page) => {
            let on = doc
                .page_of(id)
                .map(|pg| crate::private::page(doc, pg).into_owned())
                .unwrap_or_default();
            resp.on_hover_text(format!("On {on}"))
        }
        _ => resp,
    }
}

fn more_row(ui: &mut egui::Ui, n: usize) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W - 12.0, ROW_H), egui::Sense::click());
    ui::list_row_ground(ui.painter(), rect, false, resp.hovered());
    let ink = if resp.hovered() {
        theme::text::strong()
    } else {
        theme::text::dim()
    };
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{n} more"),
        egui::FontId::proportional(ui::CONTROL_PT),
        ink,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The footer: the hint, then *Cancel* and *Clean up N items*.
fn foot(ui: &mut egui::Ui, items: usize, why_not: Option<&str>) -> Option<Act> {
    let go = match items {
        0 => "Clean up".to_string(),
        n => format!("Clean up {}", ui::count(n, "item")),
    };
    let state = match why_not {
        None => FieldButton::Primary,
        Some(_) => FieldButton::Disabled,
    };
    let buttons = [("Cancel", FieldButton::Off), (go.as_str(), state)];
    match crate::settings::card_foot(ui, W, "Enter clean up · Esc cancel", &buttons, why_not) {
        Some(0) => Some(Act::Cancel),
        Some(_) => Some(Act::Run),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{Brush, Fill, NodeKind, Operation, Transaction, keyed_by_position};

    /// A rectangle on the current page, painted red when `painted`, else drawing nothing.
    fn rect(app: &mut OndinApp, painted: bool) -> NodeId {
        let s = &mut app.session;
        let id = s.ids.mint();
        let parent = s.page;
        let index = s.doc.get(parent).map_or(0, |n| n.children().len());
        let fills = match painted {
            true => keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgb8(255, 0, 0)).into(),
                visible: true,
            }]),
            false => Vec::new(),
        };
        assert!(s.commit(Transaction(vec![
            Operation::CreateNode {
                id,
                parent,
                index,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetFills { id, fills },
        ])));
        id
    }

    fn app() -> (egui::Context, OndinApp) {
        let ctx = egui::Context::default();
        let app = OndinApp::headless(&ctx);
        (ctx, app)
    }

    /// One frame of the whole app.
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

    /// With nothing selected the box opens on the page; Enter takes what it found
    /// as one undo step and keeps the switches for next time.
    /// flip: `commit_clean_up` not saving `on` → fails at "the switch is kept".
    #[test]
    fn enter_cleans_up_in_one_step_and_keeps_the_switches() {
        let (ctx, mut app) = app();
        let empty = rect(&mut app, false);
        let kept = rect(&mut app, true);
        pass(&ctx, &mut app, Vec::new());
        app.dispatch(&ctx, crate::input::Action::CleanUp);
        let b = app.clean_up.as_mut().expect("the box opened");
        assert_eq!(b.scope, Scope::Page, "nothing selected: the page");
        b.rules.set(Rule::Guides, true);
        pass(&ctx, &mut app, Vec::new());
        let b = app.clean_up.as_ref().unwrap();
        assert!(
            b.plan
                .findings
                .iter()
                .any(|f| f.subject == Subject::Layer(empty) && f.kept.is_none()),
            "fixture: the empty rect is found"
        );

        let depth = app.session.history.undo_depth();
        pass(&ctx, &mut app, vec![enter()]);
        assert!(app.clean_up.is_none(), "Enter closes the box");
        assert!(!app.session.doc.contains(empty));
        assert!(app.session.doc.contains(kept));
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
        assert!(
            app.prefs.clean_up.on.iter().any(|k| k == "guides"),
            "the switch is kept"
        );
        assert!(app.session.undo());
        assert!(app.session.doc.contains(empty), "undo puts it back");
    }

    /// Over a selection with nothing to take, the box stays on the selection,
    /// Clean up dims with its reason and Enter leaves the box up; a switched-off
    /// rule that would find something is named in the empty preview.
    #[test]
    fn nothing_to_take_dims_clean_up_and_names_what_is_off() {
        let (ctx, mut app) = app();
        let painted = rect(&mut app, true);
        assert!(app.session.commit(Transaction(vec![Operation::SetVisible {
            id: painted,
            visible: false,
        }])));
        app.session.selection.set(vec![painted]);
        pass(&ctx, &mut app, Vec::new());
        app.open_clean_up();
        pass(&ctx, &mut app, Vec::new());
        let b = app.clean_up.as_ref().expect("the box opened");
        assert_eq!(b.scope, Scope::Selection);
        assert_eq!(b.why_not(), Some("Nothing to clean up"));
        assert_eq!(
            turned_off(b).as_deref(),
            Some("Turned off: Hidden layers 1"),
            "hidden layers start off"
        );
        pass(&ctx, &mut app, vec![enter()]);
        assert!(app.clean_up.is_some(), "a dim Clean up runs nothing");
        assert!(app.session.doc.contains(painted));
    }
}
