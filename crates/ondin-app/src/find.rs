//! Find and replace in what text layers say (`Ctrl+F`, `Ctrl+H`): a box at the
//! canvas's top-right, beside the inspector and never over it, listing every
//! match in the selection, the page or every page (`ondin_core::find`), and the
//! matches tinted on the canvas, the current one ringed.
//!
//! Not a modal: the canvas stays live and the matches are found again as the
//! text changes. Stepping shows a match — the page switched, the view panned,
//! the zoom kept — and never selects it.

use crate::app::OndinApp;
use crate::batch_rename::{field_ground, scope_segments, toggles};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::find::{self, Hit, Query, Scope};
use ondin_core::kurbo::Point;
use ondin_core::text::TextParts;
use ondin_core::{NodeId, NodeKind, TextEdit, TextRef};
use std::collections::HashMap;
use std::ops::Range;

/// The box's width.
const W: f32 = 420.0;
const PAD: f32 = 12.0;
/// Its gap from the canvas's top and right edges (or the inspector's left).
const MARGIN: f32 = 16.0;
const FIELD_H: f32 = ui::CONTROL_H;
const ROW_H: f32 = 28.0;
/// The list scrolls past this many rows.
const ROWS_SHOWN: f32 = 10.0;
/// The side of the arrow and close buttons, the caret's width, and the air
/// between controls in a row.
const BTN: f32 = 26.0;
const CARET_W: f32 = 20.0;
const GAP_X: f32 = 6.0;
/// The indent under the caret, so the second rows line up with Find.
const INDENT: f32 = CARET_W + GAP_X;
/// Find's three switches inside its field, [`toggles`]' 4 apart.
const SWITCHES_W: f32 = ui::FIELD_TOGGLE.x * 3.0 + 4.0 * 2.0;
/// Between a field's leading glyph and its text.
const GLYPH_GAP: f32 = 7.0;
/// The ids of the two fields, which the keys ask about.
const FIND_ID: &str = "find-box-find";
const WITH_ID: &str = "find-box-with";

/// Where the box looks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum In {
    Selection,
    #[default]
    Page,
    All,
}

/// The box's state. Kept closed rather than dropped, so it reopens as it was
/// left: the Replace row folded or not, the switches, the last query.
#[derive(Default)]
pub(crate) struct FindBox {
    pub(crate) open: bool,
    pub(crate) find: String,
    pub(crate) with: String,
    /// The Replace row and the options under it are showing.
    pub(crate) replace_row: bool,
    pub(crate) match_case: bool,
    pub(crate) whole_words: bool,
    pub(crate) pattern: bool,
    pub(crate) keep_case: bool,
    pub(crate) scope: In,
    pub(crate) inside_instances: bool,
    /// The match the canvas rings and Replace writes, an index into `found.hits`.
    pub(crate) current: usize,
    /// The Find field takes the keyboard next frame.
    focus: bool,
    /// Opened once already, so the defaults have been set.
    seeded: bool,
    found: Found,
    /// After a Replace: the layer and the byte the next match is looked for from.
    resume: Option<(NodeId, usize)>,
    /// A node whose match was stepped to on another page, revealed once there.
    reveal: Option<NodeId>,
    /// What *All* wrote, said until the query changes.
    done: Option<String>,
    /// Each layer's match highlights in text-local space, per revision.
    quads: HashMap<NodeId, (u64, MatchQuads)>,
}

/// Each match's byte range with the quads that highlight it.
type MatchQuads = Vec<(Range<usize>, Vec<[Point; 4]>)>;

/// The matches for one key, found again only when it changes.
#[derive(Default)]
struct Found {
    key: Option<Key>,
    hits: Vec<Hit>,
    /// A pattern that does not parse, in a line.
    error: Option<String>,
}

#[derive(Clone, PartialEq)]
struct Key {
    rev: u64,
    page: NodeId,
    find: String,
    switches: (bool, bool, bool),
    scope: In,
    selection: Vec<NodeId>,
}

impl FindBox {
    /// The query as the switches stand, or why it cannot be built.
    fn query(&self) -> Result<Query, String> {
        Query::new(
            &self.find,
            &self.with,
            self.match_case,
            self.whole_words,
            self.pattern,
            self.keep_case,
        )
    }

    fn current_hit(&self) -> Option<&Hit> {
        self.found.hits.get(self.current)
    }
}

impl OndinApp {
    /// Open the box on Find (`Ctrl+F`), or with its Replace row showing (`Ctrl+H`).
    /// It looks in the selection when there is one, else on this page, and takes
    /// the keyboard. Opened again while open, it only takes the keyboard back.
    pub(crate) fn open_find(&mut self, replace: bool) {
        let f = &mut self.find;
        if !f.seeded {
            f.seeded = true;
            f.keep_case = true;
        }
        if !f.open {
            f.scope = match self.session.selection.ids().is_empty() {
                true => In::Page,
                false => In::Selection,
            };
            f.current = 0;
            f.done = None;
        }
        f.open = true;
        f.focus = true;
        if replace {
            f.replace_row = true;
        }
    }

    /// Close the box; the selection is left as it was.
    pub(crate) fn close_find(&mut self) {
        self.find.open = false;
        self.find.reveal = None;
    }

    /// Find again when the document, the page, the selection or the query moved.
    fn refresh_find(&mut self) {
        let f = &mut self.find;
        let selection = self.session.selection.ids().to_vec();
        // Selection with nothing selected looks on the page.
        let scope = match f.scope {
            In::Selection if selection.is_empty() => In::Page,
            s => s,
        };
        let key = Key {
            rev: self.session.revision(),
            page: self.session.page,
            find: f.find.clone(),
            switches: (f.match_case, f.whole_words, f.pattern),
            scope,
            selection: match scope {
                In::Selection => selection.clone(),
                _ => Vec::new(),
            },
        };
        if f.found.key.as_ref() == Some(&key) {
            return;
        }
        let query_moved = f.found.key.as_ref().is_some_and(|k| {
            k.find != key.find || k.switches != key.switches || k.scope != key.scope
        });
        let was = f.current_hit().cloned();
        let doc = &self.session.doc;
        let (hits, error) = match f.query() {
            Ok(q) => {
                let scope = match scope {
                    In::Selection => Scope::Under(selection),
                    In::Page => Scope::Page(self.session.page),
                    In::All => Scope::Pages {
                        here: self.session.page,
                    },
                };
                (find::find(doc, &scope, &q), None)
            }
            Err(e) => (Vec::new(), Some(e)),
        };
        // The current match stays the same one where it can: after a Replace the
        // first match past what it wrote, else the one it was, else its place.
        let resume = f.resume.take();
        f.current = if query_moved {
            0
        } else if let Some((node, from)) = resume {
            hits.iter()
                .position(|h| h.node == node && h.range.start >= from)
                .or_else(|| {
                    let last = hits.iter().rposition(|h| h.node == node)?;
                    Some(last + 1)
                })
                .unwrap_or(f.current)
        } else if let Some(w) = &was {
            hits.iter()
                .position(|h| h.node == w.node && h.range == w.range)
                .unwrap_or(f.current)
        } else {
            f.current
        };
        if f.current >= hits.len() {
            f.current = 0;
        }
        if query_moved {
            f.done = None;
        }
        f.found = Found {
            key: Some(key),
            hits,
            error,
        };
    }

    /// Make match `i` current and show it: its page, then the view panned to it
    /// by the least that will do, the zoom kept. Nothing is selected.
    fn show_hit(&mut self, i: usize) {
        let Some(hit) = self.find.found.hits.get(i).cloned() else {
            return;
        };
        self.find.current = i;
        if hit.page != self.session.page {
            self.go_to_page(hit.page);
        }
        self.find.reveal = Some(hit.node);
    }

    /// Step the current match by `by`, round the ends.
    fn step_find(&mut self, by: isize) {
        let n = self.find.found.hits.len();
        if n == 0 {
            return;
        }
        let at = (self.find.current as isize + by).rem_euclid(n as isize) as usize;
        self.show_hit(at);
    }

    /// Write the current match and move on to the next; a match a replace may not
    /// write is stepped over.
    fn replace_current(&mut self) {
        let Ok(query) = self.find.query() else { return };
        let Some(hit) = self.find.current_hit().cloned() else {
            return;
        };
        if !hit.writable(self.find.inside_instances) {
            self.step_find(1);
            return;
        }
        let content = match self.session.doc.get(hit.node).map(|n| n.kind()) {
            Some(NodeKind::Text { content, .. }) => content.clone(),
            _ => return,
        };
        let Some(with) = query.replacement(&content, &hit.range) else {
            return;
        };
        let done = find::replace(
            &self.session.doc,
            std::slice::from_ref(&hit),
            &query,
            self.find.inside_instances,
        );
        if self
            .session
            .commit_aside(done.tx, "Replace text", icon::TEXT_AA)
        {
            self.find.resume = Some((hit.node, hit.range.start + with.len()));
        }
    }

    /// Write every match in scope a replace may write, as one undo step.
    fn replace_all(&mut self) {
        let Ok(query) = self.find.query() else { return };
        let done = find::replace(
            &self.session.doc,
            &self.find.found.hits,
            &query,
            self.find.inside_instances,
        );
        if done.matches == 0 {
            return;
        }
        if self
            .session
            .commit_aside(done.tx, "Replace text", icon::TEXT_AA)
        {
            let said = format!(
                "Replaced {} in {} · Ctrl+Z to undo",
                done.matches,
                plural(done.layers, "layer")
            );
            self.session.info(said.clone());
            self.find.done = Some(said);
        }
    }

    /// Edit match `i`'s text with the match selected: a row's double-click.
    fn edit_hit(&mut self, i: usize) {
        let Some(hit) = self.find.found.hits.get(i).cloned() else {
            return;
        };
        self.show_hit(i);
        self.begin_edit_text(Some(hit.node), None);
        if let Some(t) = self.text.as_mut().filter(|t| t.id == hit.node) {
            t.editor.select_range(hit.range);
        }
    }

    /// Draw the box when it is open, and act on this frame's keys and clicks.
    pub(crate) fn find_ui(&mut self, ctx: &egui::Context) {
        if !self.find.open {
            return;
        }
        self.refresh_find();
        if let Some(id) = self.find.reveal
            && self.session.doc.page_of(id) == Some(self.session.page)
        {
            self.find.reveal = None;
            // The match itself, so a long text's later lines come into view; the
            // layer when it cannot be laid out.
            let hit = self.find.current_hit().filter(|h| h.node == id).cloned();
            let bounds = hit
                .and_then(|h| self.match_bounds(&h))
                .or_else(|| self.session.preview_world_bounds(id));
            if let Some(b) = bounds {
                self.reveal(b);
            }
        }

        // Asked before the fields are drawn: `Enter` steps (or replaces) while one
        // of them has the keyboard, and the fields keep it (`return_key(None)`).
        let (in_find, in_with) = ctx.memory(|m| {
            (
                m.has_focus(egui::Id::new(FIND_ID)),
                m.has_focus(egui::Id::new(WITH_ID)),
            )
        });
        let (enter, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Enter), i.modifiers.shift));
        let mut act = None;
        if enter && in_find {
            act = Some(Act::Step(if shift { -1 } else { 1 }));
        } else if enter && in_with {
            act = Some(Act::Replace);
        }

        let right = if self.focus {
            self.canvas_rect.right()
        } else {
            Self::inspector_column(ctx.content_rect()).left()
        };
        let top = self.canvas_rect.top()
            + if self.rulers_on() {
                crate::rulers::THICKNESS
            } else {
                0.0
            };
        let anchor = egui::pos2(right - MARGIN, top + MARGIN);
        let max_h = (self.canvas_rect.bottom() - anchor.y - MARGIN).max(160.0);
        let selected = self.session.selection.ids().len();
        let names: Vec<(String, String)> = self
            .find
            .found
            .hits
            .iter()
            .map(|h| {
                let page = crate::private::page(&self.session.doc, h.page).into_owned();
                let layer = crate::private::layer(&self.session.doc, h.node).into_owned();
                (page, layer)
            })
            .collect();
        let rows: Vec<RowText> = self
            .find
            .found
            .hits
            .iter()
            .map(|h| row_text(&self.session.doc, h))
            .collect();
        let f = &mut self.find;
        egui::Area::new(egui::Id::new("find-box"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(color::card())
                    .corner_radius(8)
                    .stroke(egui::Stroke::new(1.0, color::text_a(16)))
                    .shadow(egui::Shadow {
                        offset: [0, 10],
                        blur: 24,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(82),
                    })
                    .show(ui, |ui| {
                        ui.set_width(W);
                        ui.set_max_height(max_h);
                        ui.spacing_mut().item_spacing = egui::vec2(GAP_X, 8.0);
                        egui::Frame::NONE
                            .inner_margin(PAD)
                            .show(ui, |ui| controls(ui, f, selected, &mut act));
                        // Nothing under the fields until there is something to
                        // list, so the folded box is as deep below as above.
                        if f.find.is_empty() {
                            return;
                        }
                        rule(ui);
                        list(ui, f, &names, &rows, &mut act);
                        if !f.found.hits.is_empty() {
                            rule(ui);
                            footer(ui, f);
                        }
                    });
            });

        match act {
            None => {}
            Some(Act::Step(by)) => self.step_find(by),
            Some(Act::Show(i)) => self.show_hit(i),
            Some(Act::Edit(i)) => self.edit_hit(i),
            Some(Act::Replace) => self.replace_current(),
            Some(Act::All) => self.replace_all(),
            Some(Act::Widen) => {
                self.find.scope = match self.find.scope {
                    In::Selection => In::Page,
                    _ => In::All,
                };
            }
            Some(Act::Close) => self.close_find(),
        }
    }

    /// The matches on this page tinted over the canvas, the current one ringed; a
    /// match a replace would skip is grey.
    pub(crate) fn paint_find_marks(&mut self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if !self.find.open || self.find.found.hits.is_empty() {
            return;
        }
        let rev = self.session.revision();
        let page = self.session.page;
        let current = self.find.current_hit().cloned();
        let inside = self.find.inside_instances;
        let mut by_node: Vec<(NodeId, Vec<Range<usize>>)> = Vec::new();
        for h in self.find.found.hits.iter().filter(|h| h.page == page) {
            match by_node.last_mut() {
                Some((n, rs)) if *n == h.node => rs.push(h.range.clone()),
                _ => by_node.push((h.node, vec![h.range.clone()])),
            }
        }
        for (node, ranges) in by_node {
            let fresh = self
                .find
                .quads
                .get(&node)
                .is_some_and(|(r, q)| *r == rev && q.iter().map(|(r, _)| r).eq(ranges.iter()));
            if !fresh {
                let quads = self.match_quads(node, &ranges);
                self.find.quads.insert(node, (rev, quads));
            }
            let Some(world) = self.session.preview_world_transform(node) else {
                continue;
            };
            let Some((_, quads)) = self.find.quads.get(&node) else {
                continue;
            };
            let hit_of = |r: &Range<usize>| {
                self.find
                    .found
                    .hits
                    .iter()
                    .find(|h| h.node == node && h.range == *r)
            };
            for (r, qs) in quads {
                let writable = hit_of(r).is_some_and(|h| h.writable(inside));
                let is_current = current
                    .as_ref()
                    .is_some_and(|c| c.node == node && c.range == *r);
                let fill = if writable {
                    color::accent().gamma_multiply_u8(77)
                } else {
                    color::text_a(56)
                };
                for q in qs {
                    let pts: Vec<egui::Pos2> = q
                        .iter()
                        .map(|p| self.to_screen(world * *p, rect, ppp))
                        .collect();
                    let stroke = match is_current {
                        true => egui::Stroke::new(1.5, color::accent()),
                        false => egui::Stroke::NONE,
                    };
                    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
                }
            }
        }
        // A layer gone from the matches drops its highlights.
        let live: Vec<NodeId> = self.find.found.hits.iter().map(|h| h.node).collect();
        self.find.quads.retain(|n, _| live.contains(n));
    }

    /// A match's highlight bounds in world space.
    fn match_bounds(&self, hit: &Hit) -> Option<ondin_core::kurbo::Rect> {
        let world = self.session.preview_world_transform(hit.node)?;
        self.match_quads(hit.node, std::slice::from_ref(&hit.range))
            .into_iter()
            .flat_map(|(_, qs)| qs)
            .flatten()
            .map(|p| {
                let p = world * p;
                ondin_core::kurbo::Rect::from_points(p, p)
            })
            .reduce(|a, b| a.union(b))
    }

    /// The highlight quads of each of `ranges` in `id`'s text, laid out as the
    /// canvas draws it.
    fn match_quads(&self, id: NodeId, ranges: &[Range<usize>]) -> MatchQuads {
        let Some(kind) = self.session.doc.resolved_kind(id) else {
            return Vec::new();
        };
        let (Some(parts), Some(content)) = (TextParts::of(&kind), TextRef::of(&kind)) else {
            return Vec::new();
        };
        let editor = TextEdit::new(content.content.to_owned(), parts);
        ranges
            .iter()
            .map(|r| (r.clone(), editor.range_quads(r.clone())))
            .collect()
    }
}

/// What a frame of the box asked for.
enum Act {
    Step(isize),
    Show(usize),
    Edit(usize),
    Replace,
    All,
    /// The empty list's button: Selection → This page → All pages.
    Widen,
    Close,
}

/// The rows above the list: Find with its count, switches and arrows, then —
/// unfolded — Replace, the scope and the instances checkbox.
fn controls(ui: &mut egui::Ui, f: &mut FindBox, selected: usize, act: &mut Option<Act>) {
    let n = f.found.hits.len();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (caret, tip) = match f.replace_row {
            true => (icon::CARET_DOWN, "Hide replace"),
            false => (icon::CARET_RIGHT, "Show replace"),
        };
        // The whole button a point low: centred, it reads high beside the field.
        let (slot, _) = ui.allocate_exact_size(egui::vec2(CARET_W, FIELD_H), egui::Sense::hover());
        let mut child = ui.new_child(
            egui::UiBuilder::new().max_rect(
                egui::Rect::from_center_size(slot.center(), egui::vec2(CARET_W, BTN))
                    .translate(egui::vec2(0.0, 1.0)),
            ),
        );
        if ui::icon_button_sized(
            &mut child,
            caret,
            egui::vec2(CARET_W, BTN),
            12.0,
            false,
            true,
        )
        .on_hover_text(tip)
        .clicked()
        {
            f.replace_row = !f.replace_row;
        }
        ui.add_space(GAP_X);
        let w = ui.available_width() - BTN * 3.0 - GAP_X * 2.0 - 2.0;
        let count = if f.find.is_empty() {
            String::new()
        } else if f.found.error.is_some() {
            "Not a pattern".into()
        } else if n == 0 {
            // A word here took the field's room; the warm ink and the line
            // under the field already say it.
            "0".into()
        } else {
            format!("{} of {n}", f.current + 1)
        };
        // Warm when nothing was found; after *All* emptied the list it is news.
        let warm = n == 0 && !f.find.is_empty() && f.done.is_none();
        let (resp, rect) = field(
            ui,
            FIND_ID,
            &mut f.find,
            "Find in text",
            icon::MAGNIFYING_GLASS,
            w,
            &count,
            warm,
            SWITCHES_W,
        );
        // Inside the field, as Batch rename's are.
        let mut child = toggles(ui, rect, SWITCHES_W);
        ui::field_toggle(&mut child, "Aa", "Match case", &mut f.match_case, false);
        ui::field_toggle(&mut child, "ab", "Whole words", &mut f.whole_words, false);
        ui::field_toggle(&mut child, ".*", "Regular expression", &mut f.pattern, true);
        if f.focus {
            f.focus = false;
            resp.request_focus();
            ui::select_all_on_focus(ui, &resp, &f.find);
        }
        let live = n > 0;
        ui.add_space(GAP_X);
        if ui::icon_button(ui, icon::ARROW_UP, BTN, 14.0, false, live)
            .on_hover_text("Previous match (Shift+Enter)")
            .clicked()
            && live
        {
            *act = Some(Act::Step(-1));
        }
        ui.add_space(2.0);
        // Its ink sits high in the em box; the button stays in line.
        if ui::icon_button_nudged(
            ui,
            icon::ARROW_DOWN,
            egui::vec2(BTN, BTN),
            14.0,
            live,
            egui::vec2(0.0, 1.0),
        )
        .on_hover_text("Next match (Enter)")
        .clicked()
            && live
        {
            *act = Some(Act::Step(1));
        }
        ui.add_space(GAP_X);
        if ui::icon_button(ui, icon::X, BTN, 13.0, false, true)
            .on_hover_text("Close (Escape)")
            .clicked()
        {
            *act = Some(Act::Close);
        }
    });
    if let Some(e) = &f.found.error {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(INDENT);
            ui.label(egui::RichText::new(e).size(10.5).color(color::warn()));
        });
    }
    if !f.replace_row {
        return;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP_X;
        ui.add_space(INDENT);
        let live = n > 0 && f.found.error.is_none();
        let state = |primary| match (live, primary) {
            (false, _) => FieldButton::Disabled,
            (true, true) => FieldButton::Primary,
            (true, false) => FieldButton::Off,
        };
        let (replace_w, all_w) = (button_w(ui, "Replace"), button_w(ui, "All"));
        let w = ui.available_width() - replace_w - all_w - GAP_X * 2.0;
        let keep_case_w = ui::FIELD_TOGGLE.x;
        let (_, rect) = field(
            ui,
            WITH_ID,
            &mut f.with,
            "Replace with",
            icon::SWAP,
            w,
            "",
            false,
            keep_case_w,
        );
        let mut child = toggles(ui, rect, keep_case_w);
        ui::field_toggle(
            &mut child,
            "Ab",
            "Keep each match’s case",
            &mut f.keep_case,
            false,
        );
        if crate::settings::text_button(ui, "Replace", state(false)).clicked() && live {
            *act = Some(Act::Replace);
        }
        if crate::settings::text_button(ui, "All", state(true))
            .on_hover_text("Replace every match in scope")
            .clicked()
            && live
        {
            *act = Some(Act::All);
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(INDENT);
        const SCOPES: [In; 3] = [In::Selection, In::Page, In::All];
        // A Selection scope with nothing selected looks at the page, and says so.
        let lit = match f.scope {
            In::Selection if selected == 0 => 1,
            scope => SCOPES.iter().position(|x| *x == scope).unwrap_or(1),
        };
        if let Some(i) = scope_segments(ui, lit, selected, ui::SEGMENT_CELL_H) {
            f.scope = SCOPES[i];
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.add_space(INDENT);
        ui::tick_box(
            ui,
            "Include text inside instances",
            "",
            &mut f.inside_instances,
        );
    });
}

/// The width [`crate::settings::text_button`] gives `label`: its word at
/// [`ui::CONTROL_PT`] and 14 either side.
fn button_w(ui: &egui::Ui, label: &str) -> f32 {
    let g = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            label.to_string(),
            egui::FontId::proportional(ui::CONTROL_PT),
            color::text(),
        )
    });
    g.size().x + 28.0
}

/// The list: matches grouped by page, a long list scrolling inside the box. With
/// none, what was looked for and the step that widens the scope.
fn list(
    ui: &mut egui::Ui,
    f: &FindBox,
    names: &[(String, String)],
    rows: &[RowText],
    act: &mut Option<Act>,
) {
    if f.find.is_empty() {
        return;
    }
    if f.found.hits.is_empty() {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(PAD as i8, 14))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                if let Some(done) = &f.done {
                    ui.label(
                        egui::RichText::new(format!("{} {done}", icon::CHECK))
                            .size(11.5)
                            .color(theme::text::muted()),
                    );
                    return;
                }
                let place = match f.scope {
                    In::Selection => "the selection",
                    In::Page => "this page",
                    In::All => "any page",
                };
                ui.label(
                    egui::RichText::new(format!("Nothing in {place} matches."))
                        .size(12.0)
                        .color(theme::text::muted()),
                );
                let widen = match f.scope {
                    In::Selection => Some("Search this page"),
                    In::Page => Some("Search all pages"),
                    In::All => None,
                };
                if let Some(label) = widen
                    && f.found.error.is_none()
                    && crate::settings::text_button(ui, label, FieldButton::Off).clicked()
                {
                    *act = Some(Act::Widen);
                }
            });
        return;
    }
    // At most `ROWS_SHOWN` rows, fewer when the canvas is short; the wheel scrolls
    // it and no bar is drawn.
    let most = (ROW_H + 1.0) * ROWS_SHOWN + 16.0;
    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .max_height(most.min(ui.available_height().max(ROW_H * 3.0) - 40.0))
        .show(ui, |ui| {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(6, 8))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let mut page = None;
                    for (i, h) in f.found.hits.iter().enumerate() {
                        if page != Some(h.page) {
                            page = Some(h.page);
                            let count = f.found.hits.iter().filter(|o| o.page == h.page).count();
                            page_head(ui, &names[i].0, count);
                        }
                        let current = i == f.current;
                        let resp =
                            result_row(ui, h, &names[i].1, &rows[i], current, f.inside_instances);
                        if current && f.reveal.is_none() {
                            resp.scroll_to_me(None);
                        }
                        if crate::ui::double_clicked(ui, &resp) {
                            *act = Some(Act::Edit(i));
                        } else if resp.clicked() {
                            *act = Some(Act::Show(i));
                        }
                    }
                });
        });
}

/// What a result row reads: the text around the match, cut at both ends, and
/// whether the layer draws its text case-transformed.
struct RowText {
    pre: String,
    hit: String,
    post: String,
    cased: bool,
}

/// How much text a row keeps before a match.
const CONTEXT: usize = 18;

fn row_text(doc: &ondin_core::Document, h: &Hit) -> RowText {
    let Some(NodeKind::Text {
        content,
        style,
        spans,
        ..
    }) = doc.get(h.node).map(|n| n.kind())
    else {
        return RowText {
            pre: String::new(),
            hit: String::new(),
            post: String::new(),
            cased: false,
        };
    };
    let before = &content[..h.range.start];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let before = &before[line_start..];
    let skip = before.chars().count().saturating_sub(CONTEXT);
    let pre: String = before.chars().skip(skip).collect();
    let pre = if skip > 0 { format!("…{pre}") } else { pre };
    let after = &content[h.range.end..];
    let post = after.lines().next().unwrap_or_default().to_string();
    RowText {
        pre,
        hit: content[h.range.clone()].replace('\n', " "),
        post,
        cased: spans.resolve(style, h.range.start).case != ondin_core::TextCase::Original,
    }
}

/// A page's heading in the list, its match count on the right.
fn page_head(ui: &mut egui::Ui, name: &str, count: usize) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::hover());
    let p = ui.painter();
    p.text(
        rect.left_center() + egui::vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name.to_uppercase(),
        egui::FontId::proportional(9.5),
        color::text_a(97),
    );
    p.text(
        rect.right_center() - egui::vec2(6.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(10.5),
        color::text_a(97),
    );
}

/// One match: the kind's glyph, the text around the match with the match lit, the
/// layer's name, and its state — a lock, a slashed eye, a case transform. Dim
/// when a replace would skip it.
fn result_row(
    ui: &mut egui::Ui,
    h: &Hit,
    layer: &str,
    t: &RowText,
    current: bool,
    inside: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let mut p = ui.painter().clone();
    if !h.writable(inside) {
        p.multiply_opacity(0.45);
    }
    ui::list_row_ground(&p, rect, current, resp.hovered());
    let y = rect.center().y;
    let gx = rect.left() + 13.0;
    if h.main.is_some() {
        crate::ui::paint_hexagon_filled(&p, egui::pos2(gx, y), 12.0, color::accent_400());
    } else {
        let glyph = if h.instance.is_some() {
            icon::HEXAGON
        } else {
            icon::TEXT_T
        };
        p.text(
            egui::pos2(gx, y),
            egui::Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(14.0),
            theme::text::dim(),
        );
    }
    let tail = if h.locked {
        Some(icon::LOCK_SIMPLE)
    } else if h.hidden {
        Some(icon::EYE_SLASH)
    } else if t.cased {
        Some(icon::TEXT_AA)
    } else {
        None
    };
    let mut right = rect.right() - 6.0;
    if let Some(g) = tail {
        p.text(
            egui::pos2(right - 6.0, y),
            egui::Align2::CENTER_CENTER,
            g,
            theme::icon_font(13.0),
            theme::text::dim(),
        );
        right -= 20.0;
    }
    let layer = match h.main {
        Some((_, copies)) if copies > 0 => format!("{layer} · {copies}"),
        _ => layer.to_string(),
    };
    let name = ui.fonts_mut(|fo| {
        fo.layout_job(egui::text::LayoutJob {
            wrap: egui::text::TextWrapping::truncate_at_width(96.0),
            ..egui::text::LayoutJob::simple_singleline(
                layer,
                egui::FontId::proportional(10.5),
                color::text_a(97),
            )
        })
    });
    let name_w = name.size().x;
    p.galley(
        egui::pos2(right - name_w, y - name.size().y / 2.0),
        name,
        color::text_a(97),
    );
    let x = rect.left() + 27.0;
    let room = (right - name_w - 8.0 - x).max(10.0);
    let font = egui::FontId::proportional(12.0);
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping::truncate_at_width(room),
        ..Default::default()
    };
    let dim = egui::TextFormat::simple(font.clone(), color::text_a(179));
    job.append(&t.pre, 0.0, dim.clone());
    job.append(
        &t.hit,
        0.0,
        egui::TextFormat {
            color: color::text(),
            background: color::accent_a(72),
            ..egui::TextFormat::simple(font, color::text())
        },
    );
    job.append(&t.post, 0.0, dim);
    let line = ui.fonts_mut(|fo| fo.layout_job(job));
    p.galley(egui::pos2(x, y - line.size().y / 2.0), line, color::text());
    resp
}

/// The status line under the list: how many matches in how many layers, and what
/// a replace would skip.
fn footer(ui: &mut egui::Ui, f: &FindBox) {
    let hits = &f.found.hits;
    let mut layers: Vec<NodeId> = hits.iter().map(|h| h.node).collect();
    layers.sort();
    layers.dedup();
    let mut said = format!(
        "{} in {}",
        plural(hits.len(), "match"),
        plural(layers.len(), "layer")
    );
    let skipped = hits
        .iter()
        .filter(|h| !h.locked && h.instance.is_some() && !f.inside_instances)
        .count();
    if skipped > 0 {
        said.push_str(&format!(" · {skipped} inside an instance skipped"));
    }
    let locked = hits.iter().filter(|h| h.locked).count();
    if locked > 0 {
        said.push_str(&format!(" · {locked} locked"));
    }
    if let Some(done) = &f.done {
        said = done.clone();
    }
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(PAD as i8, 9))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(said)
                    .size(11.0)
                    .color(theme::text::faint()),
            );
        });
}

fn plural(n: usize, what: &str) -> String {
    match (n, what) {
        (1, w) => format!("1 {w}"),
        (n, "match") => format!("{n} matches"),
        (n, w) => format!("{n} {w}s"),
    }
}

/// A field with a glyph before it and an optional note at its end, `w` wide,
/// `toggles_w` left free at its right for [`toggles`]; with the rect it fills.
#[allow(clippy::too_many_arguments)]
fn field(
    ui: &mut egui::Ui,
    id: &str,
    text: &mut String,
    hint: &str,
    glyph: &str,
    w: f32,
    note: &str,
    warm: bool,
    toggles_w: f32,
) -> (egui::Response, egui::Rect) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w.max(60.0), FIELD_H), egui::Sense::hover());
    let id = egui::Id::new(id);
    field_ground(ui, rect, ui.memory(|m| m.has_focus(id)));
    let p = ui.painter();
    p.text(
        rect.left_center() + egui::vec2(ui::FIELD_PAD_X + ui::CONTROL_GLYPH_PT / 2.0, 0.0),
        egui::Align2::CENTER_CENTER,
        glyph,
        theme::icon_font(ui::CONTROL_GLYPH_PT),
        theme::text::faint(),
    );
    let end = if toggles_w > 0.0 {
        rect.right() - 8.0 - toggles_w
    } else {
        rect.right() - ui::FIELD_PAD_X
    };
    let note_w = if note.is_empty() {
        0.0
    } else {
        let ink = if warm {
            color::warn()
        } else {
            theme::text::dim()
        };
        let g = ui.fonts_mut(|f| {
            f.layout_no_wrap(note.to_string(), egui::FontId::proportional(11.0), ink)
        });
        let w = g.size().x;
        p.galley(
            egui::pos2(end - w, rect.center().y - g.size().y / 2.0),
            g,
            ink,
        );
        w + 8.0
    };
    // From the field's left edge, the glyph's band in the edit's padding, so a
    // press anywhere left of the note focuses it.
    let lead = ui::FIELD_PAD_X + ui::CONTROL_GLYPH_PT + GLYPH_GAP;
    let inner = egui::Rect::from_min_max(rect.min, egui::pos2(end - note_w, rect.bottom()));
    let font = egui::FontId::proportional(ui::CONTROL_PT);
    let mut frame = ui::edit_frame(ui, &font, rect.height(), 0.0);
    frame.inner_margin.left = lead as i8;
    // `place`, not `put`: in a row, `put` leaves the cursor at the text's end,
    // short of the note, and the buttons after the field land on it.
    #[expect(
        clippy::disallowed_methods,
        reason = "the entry inside a composed field row, which paints the ground and ui::typing_edge itself"
    )]
    let resp = ui.place(
        inner,
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(frame)
            .desired_width(inner.width() - lead)
            .return_key(None)
            .font(font)
            .text_color(color::text())
            .hint_text(
                egui::RichText::new(hint)
                    .size(ui::CONTROL_PT)
                    .color(theme::text::faint()),
            ),
    );
    (resp, rect)
}

/// A hairline across the box.
fn rule(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, color::text_a(20));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{Operation, TextSizing, TextStyle, Transaction};

    fn text(content: &str) -> NodeKind {
        NodeKind::Text {
            content: content.into(),
            style: Box::new(TextStyle {
                font_family: "Inter".into(),
                font_size: 12.0,
                ..Default::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    fn content(app: &OndinApp, id: NodeId) -> String {
        match app.session.doc.get(id).unwrap().kind() {
            NodeKind::Text { content, .. } => content.clone(),
            _ => unreachable!(),
        }
    }

    /// A headline with two *team*s and a locked note on this page; a hint on a
    /// second page.
    struct F {
        app: OndinApp,
        first: NodeId,
        second: NodeId,
        headline: NodeId,
        note: NodeId,
        hint: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> F {
        let mut app = OndinApp::headless(ctx);
        let first = app.session.page;
        let [second, headline, note, hint] = [(); 4].map(|_| app.session.ids.mint());
        let create = |id, parent, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind,
            transform: None,
            name: Some(name.into()),
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::AddPage {
                id: second,
                index: 1,
                name: "Checkout".into(),
                ground: None,
                draft: false,
                mode: None,
            },
            // The headline on top of the stack, so it is listed first.
            create(headline, first, text("A team for every team"), "Headline"),
            create(note, first, text("Talk to the team."), "Note"),
            Operation::SetLocked {
                id: note,
                locked: true,
            },
            create(hint, second, text("Invite your team later"), "Hint"),
        ])));
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 700.0));
        F {
            app,
            first,
            second,
            headline,
            note,
            hint,
        }
    }

    fn run(app: &mut OndinApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            // egui reads the held modifiers from the input, not from the key event.
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
        let _ = ctx.run_ui(input, |_| app.find_ui(ctx));
    }

    fn enter(shift: bool) -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: if shift {
                egui::Modifiers::SHIFT
            } else {
                egui::Modifiers::NONE
            },
        }
    }

    /// **`Ctrl+H` opens on this page with Replace showing; *All* writes every
    /// writable match as one undo step and says so**: the locked note is listed,
    /// never written, and one undo puts both of the headline's back.
    ///
    /// flip: `Hit::writable` ignoring `locked` → fails at "locked, never written".
    #[test]
    fn replace_all_writes_the_page_in_one_step_and_skips_the_locked() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        f.app.open_find(true);
        assert!(f.app.find.replace_row && f.app.find.scope == In::Page);
        f.app.find.find = "team".into();
        f.app.find.with = "crew".into();
        run(&mut f.app, &ctx, Vec::new());
        let nodes: Vec<NodeId> = f.app.find.found.hits.iter().map(|h| h.node).collect();
        assert_eq!(
            nodes,
            vec![f.headline, f.headline, f.note],
            "this page only"
        );

        f.app.replace_all();
        assert_eq!(content(&f.app, f.headline), "A crew for every crew");
        assert_eq!(
            content(&f.app, f.note),
            "Talk to the team.",
            "locked, never written"
        );
        assert_eq!(
            content(&f.app, f.hint),
            "Invite your team later",
            "another page"
        );
        assert_eq!(
            f.app.find.done.as_deref(),
            Some("Replaced 2 in 1 layer · Ctrl+Z to undo")
        );
        run(&mut f.app, &ctx, Vec::new());
        assert_eq!(
            f.app.find.found.hits.len(),
            1,
            "what was skipped stays listed"
        );
        assert!(f.app.session.undo());
        assert_eq!(
            content(&f.app, f.headline),
            "A team for every team",
            "one step"
        );
    }

    /// **Replace writes the current match and moves to the next**, even when the
    /// replacement itself matches: *team* → *teams* steps past what it wrote.
    ///
    /// flip: `replace_current` not setting `resume` → fails at "moved past it".
    #[test]
    fn replace_writes_the_current_match_and_moves_on() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        f.app.open_find(true);
        f.app.find.find = "team".into();
        f.app.find.with = "teams".into();
        run(&mut f.app, &ctx, Vec::new());
        assert_eq!(f.app.find.current, 0);
        f.app.replace_current();
        run(&mut f.app, &ctx, Vec::new());
        assert_eq!(content(&f.app, f.headline), "A teams for every team");
        let h = &f.app.find.found.hits[f.app.find.current];
        assert_eq!(
            (h.node, h.range.clone()),
            (f.headline, 18..22),
            "moved past it"
        );
    }

    /// **`Enter` in Find steps to the next match, `Shift+Enter` back, round the
    /// ends; across pages a step switches the page and nothing is selected.**
    #[test]
    fn enter_steps_through_the_matches_and_across_pages() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        f.app.open_find(false);
        f.app.find.find = "team".into();
        f.app.find.scope = In::All;
        run(&mut f.app, &ctx, Vec::new());
        run(&mut f.app, &ctx, Vec::new());
        let field = egui::Id::new(FIND_ID);
        assert!(
            ctx.memory(|m| m.has_focus(field)),
            "the field has the keyboard"
        );
        assert_eq!(f.app.find.found.hits.len(), 4);
        run(&mut f.app, &ctx, vec![enter(true)]);
        assert_eq!(f.app.find.current, 3, "back round the start");
        assert_eq!(f.app.session.page, f.second, "the hint's page");
        assert!(f.app.session.selection.ids().is_empty(), "nothing selected");
        assert!(
            ctx.memory(|m| m.has_focus(field)),
            "Enter keeps the keyboard"
        );
        run(&mut f.app, &ctx, vec![enter(false)]);
        assert_eq!(f.app.find.current, 0);
        assert_eq!(f.app.session.page, f.first);
    }

    /// **A row's double-click edits that text with the match selected**, and
    /// `Escape` then finishes the edit before it closes the box.
    #[test]
    fn editing_a_match_selects_it_and_escape_closes_one_thing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        f.app.open_find(false);
        f.app.find.find = "every".into();
        run(&mut f.app, &ctx, Vec::new());
        f.app.edit_hit(0);
        let t = f.app.text.as_ref().expect("editing");
        assert_eq!((t.id, t.editor.selected_range()), (f.headline, 11..16));
        f.app.escape(&ctx);
        assert!(f.app.text.is_none() && f.app.find.open, "the edit first");
        f.app.escape(&ctx);
        assert!(!f.app.find.open, "then the box");
    }

    /// **Each match's highlight lies over its own glyphs**: the second *team* of
    /// the headline sits right of the first, on the same line.
    #[test]
    fn a_matchs_highlight_is_laid_out_where_its_glyphs_are() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let f = fixture(&ctx);
        let quads = f.app.match_quads(f.headline, &[2..6, 17..21]);
        let x = |i: usize| quads[i].1[0][0].x;
        let y = |i: usize| quads[i].1[0][0].y;
        assert_eq!(quads.len(), 2);
        assert!(x(1) > x(0) + 10.0, "{quads:?}");
        assert!((y(1) - y(0)).abs() < 0.5);
    }

    /// **The box keeps its width, every row unfolded and a long query in Find,
    /// and stands left of the inspector**, its top under the canvas's.
    #[test]
    fn the_box_keeps_its_width_beside_the_inspector() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        f.app.canvas_rect =
            egui::Rect::from_min_max(egui::pos2(0.0, 80.0), egui::pos2(1400.0, 900.0));
        f.app.open_find(true);
        f.app.find.find = "team".repeat(20);
        f.app.find.with = "crew".repeat(20);
        for _ in 0..3 {
            run(&mut f.app, &ctx, Vec::new());
        }
        f.app.find.find = "team".into();
        for _ in 0..3 {
            run(&mut f.app, &ctx, Vec::new());
        }
        let r = ctx
            .memory(|m| m.area_rect(egui::Id::new("find-box")))
            .expect("drawn");
        let column = OndinApp::inspector_column(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1400.0, 900.0),
        ));
        assert!((r.width() - W).abs() <= 2.0, "{r:?}");
        assert!((r.right() - (column.left() - MARGIN)).abs() < 0.5, "{r:?}");
        assert!(r.top() >= 80.0 + MARGIN, "{r:?}");
    }

    /// **The box's layout holds**: folded and empty it is as deep below Find as
    /// above; the arrows stand clear of the count; a long list stops at
    /// `ROWS_SHOWN` rows rather than running down the canvas.
    ///
    /// flip: `field` adding its text with `ui.put` → fails at the arrow's
    /// distance from the count.
    #[test]
    fn the_box_is_laid_out_clear_and_bounded() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut f = fixture(&ctx);
        let area = || {
            ctx.memory(|m| m.area_rect(egui::Id::new("find-box")))
                .expect("drawn")
        };
        f.app.open_find(false);
        for _ in 0..3 {
            run(&mut f.app, &ctx, Vec::new());
        }
        let folded = area();
        assert!(
            (folded.height() - (FIELD_H + PAD * 2.0)).abs() <= 2.5,
            "{folded:?}"
        );

        f.app.find.find = "team".into();
        let mut out = None;
        for _ in 0..3 {
            out = Some(drawn(&mut f.app, &ctx));
        }
        let painted = texts(&out.unwrap());
        let at = |s: &str| {
            painted
                .iter()
                .find(|(t, _)| t == s)
                .unwrap_or_else(|| panic!("{s}: {painted:?}"))
                .1
        };
        let (count, up) = (at("1 of 3"), at(icon::ARROW_UP));
        assert!(up.left() >= count.right() + 9.0, "{count:?} {up:?}");

        let page = f.app.session.page;
        let many: Vec<Operation> = (0..40)
            .map(|i| Operation::CreateNode {
                id: f.app.session.ids.mint(),
                parent: page,
                index: 0,
                kind: text("team"),
                transform: None,
                name: Some(format!("Copy {i}")),
            })
            .collect();
        assert!(f.app.session.commit(Transaction(many)));
        for _ in 0..3 {
            run(&mut f.app, &ctx, Vec::new());
        }
        assert_eq!(f.app.find.found.hits.len(), 43);
        let long = area();
        assert!(long.height() < 500.0, "{long:?}");
    }

    /// One frame of the box, its output kept.
    fn drawn(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |_| app.find_ui(ctx))
    }

    /// Every text the frame painted, with where it was painted.
    fn texts(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        let mut v = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut v);
        }
        v
    }
}
