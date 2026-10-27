//! Contrast check mode: every shown text layer on the page (or inside the
//! selection the mode started with) measured against what is drawn behind it,
//! with a badge on each and a fix card one click from a failing one. The
//! measurement is `ondin_export::contrast`'s, as `ondin check contrast` runs it,
//! on a worker thread.
//!
//! *View ▸ Check contrast* or the palette, no chord. It is view state: off when a
//! document opens, never saved, never exported, and present mode (its own branch
//! of the frame) never draws it. The level and the two switches are preferences
//! (`prefs.contrast`).

use std::collections::{HashMap, HashSet};

use ondin_core::kurbo::{Point, Rect};
use ondin_core::peniko::Color;
use ondin_core::{Brush, BrushSource, CharAttr, Document, Fill, NodeId, Operation, Transaction};
use ondin_export::contrast::{self, Finding, Level, Options, Report};

use crate::app::OndinApp;
use crate::job::Job;
use crate::theme::{self, color, icon};

/// How long the document must sit still after an edit before it is measured
/// again, so a run of nudges measures once.
const SETTLE: f64 = 0.15;
/// How many layers one worker job measures.
const CHUNK: usize = 6;
/// A failure's ink: its badge's ground and its outline. Red is never alone: the
/// badge carries the warning glyph and the layer the outline.
const FAIL: egui::Color32 = egui::Color32::from_rgb(0xd2, 0x3f, 0x45);
/// A failing badge's ground under the pointer.
const FAIL_HOVER: egui::Color32 = egui::Color32::from_rgb(0xe4, 0x52, 0x58);
const FAIL_LINE: egui::Color32 = egui::Color32::from_rgb(0xf0, 0x6a, 0x6f);
const PASS: egui::Color32 = egui::Color32::from_rgb(0x5c, 0xc0, 0x8a);
/// A pass or skip chip's ground: the canvas's dark, mostly opaque, so the chip
/// reads over any artwork.
const CHIP: egui::Color32 = egui::Color32::from_rgba_premultiplied(25, 25, 28, 209);
const BADGE_H: f32 = 19.0;
const BADGE_PT: f32 = 10.5;
/// A badge's distance above the layer it names.
const BADGE_LIFT: f32 = 3.0;
/// The fix card's width.
const CARD_W: f32 = 270.0;
/// The most colour variables the fix card lists.
const MAX_VARS: usize = 6;

/// The mode while it is on (`OndinApp::contrast`).
pub(crate) struct Contrast {
    /// The page being checked; switching pages starts over on the new one.
    page: NodeId,
    /// The layers whose text is checked, fixed when the mode starts; empty is the
    /// whole page.
    scope: Vec<NodeId>,
    /// The scope as the bar names it.
    scope_name: Option<String>,
    /// The layers measured, in reading order.
    subjects: Vec<NodeId>,
    marks: HashMap<NodeId, Mark>,
    /// Layers whose mark is out of date: drawn dimmed until measured again.
    stale: HashSet<NodeId>,
    /// Layers waiting to be sent to the worker.
    queue: HashSet<NodeId>,
    job: Option<Running>,
    /// The document as last seen, to tell which layers an edit could have changed.
    seen: Option<Snapshot>,
    /// When the document last changed, in egui's clock.
    changed_at: f64,
    /// When the whole run under way (the first look, or a level change) began,
    /// in egui's clock; the bar says "Measuring…" until it ends. An edit's
    /// remeasure is not one: its badges spin instead.
    measuring: Option<f64>,
    options: Options,
    /// The failure the arrows last stepped to.
    at: Option<NodeId>,
    fix: Option<FixCard>,
    level_menu: bool,
    /// The level head's bottom left, which the level menu hangs from.
    menu_at: egui::Pos2,
    /// Set on the frame the level head is clicked, whose click is not a click
    /// away from the menu it opens.
    menu_opening: bool,
    /// The badges drawn last frame, for tests to aim at.
    #[cfg(test)]
    badges_seen: Vec<(NodeId, egui::Rect)>,
}

/// What the check says about one layer.
#[derive(Clone, Debug)]
enum Mark {
    Measured(Finding),
    Skipped(&'static str),
}

struct Running {
    job: Job<Report>,
    ids: Vec<NodeId>,
    options: Options,
}

struct Snapshot {
    revision: u64,
    doc: Document,
    bounds: HashMap<NodeId, Rect>,
}

/// The fix card over one failing layer.
struct FixCard {
    id: NodeId,
    /// Where the card hangs, in screen points: beside the badge it opened from.
    anchor: egui::Pos2,
    /// The finding the rows were worked out for; a remeasure redoes them.
    finding: Finding,
    nearest: Option<Color>,
    vars: Vec<(ondin_core::VarId, String, Color, f64)>,
    /// The row previewed on the canvas.
    previewing: Option<Fix>,
    /// Set on the frame the card opens, whose click (on a badge) is not a click
    /// away from it.
    opening: bool,
    /// An Enter this frame, taken from the keymap by `contrast_claim_keys`.
    enter: bool,
}

/// A fix the card offers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fix {
    Colour(Color),
    Var(ondin_core::VarId),
}

impl Contrast {
    fn new(page: NodeId, scope: Vec<NodeId>, scope_name: Option<String>, options: Options) -> Self {
        Self {
            page,
            scope,
            scope_name,
            subjects: Vec::new(),
            marks: HashMap::new(),
            stale: HashSet::new(),
            queue: HashSet::new(),
            job: None,
            seen: None,
            changed_at: f64::NEG_INFINITY,
            measuring: None,
            options,
            at: None,
            fix: None,
            level_menu: false,
            menu_at: egui::Pos2::ZERO,
            menu_opening: false,
            #[cfg(test)]
            badges_seen: Vec::new(),
        }
    }

    /// The failing layers in reading order.
    fn failures(&self) -> Vec<NodeId> {
        self.subjects
            .iter()
            .copied()
            .filter(|id| matches!(self.marks.get(id), Some(Mark::Measured(f)) if !f.passes()))
            .collect()
    }

    /// Whether the fix card or the level menu is open, either of which an Escape
    /// closes before it reaches the canvas.
    pub(crate) fn owns_escape(&self) -> bool {
        self.fix.is_some() || self.level_menu
    }

    fn measured(&self) -> usize {
        self.subjects
            .iter()
            .filter(|id| matches!(self.marks.get(id), Some(Mark::Measured(_))))
            .count()
    }
}

impl crate::prefs::ContrastPrefs {
    /// The options the preferences ask for.
    pub(crate) fn options(&self) -> Options {
        Options {
            level: match self.level.as_str() {
                "aaa" => Level::Aaa,
                "apca" => Level::Apca,
                _ => Level::Aa,
            },
            non_text: self.non_text,
        }
    }

    fn set_level(&mut self, level: Level) {
        self.level = match level {
            Level::Aa => "aa",
            Level::Aaa => "aaa",
            Level::Apca => "apca",
        }
        .into();
    }
}

impl OndinApp {
    /// *View ▸ Check contrast*: start the mode over the selection, or the page when
    /// nothing is selected, or end it.
    pub(crate) fn toggle_contrast(&mut self) {
        if self.contrast.is_some() {
            self.end_contrast();
            return;
        }
        let (scope, scope_name) = self.contrast_target();
        self.contrast = Some(Contrast::new(
            self.session.page,
            scope,
            scope_name,
            self.prefs.contrast.options(),
        ));
    }

    /// What a check covers, and its name for the bar: the selection, cut to the
    /// top-level frame of its first layer; with nothing selected, the top-level
    /// frame filling most of the view, else the page's first in reading order.
    /// One frame at most, since measuring renders every layer's box twice. An
    /// empty scope (a page with no frame) is the whole page.
    pub(crate) fn contrast_target(&self) -> (Vec<NodeId>, Option<String>) {
        let doc = &self.session.doc;
        let page = self.session.page;
        let name = |id: NodeId| doc.get(id).map(|n| n.name().to_string());
        let sel = self.session.selection.ids();
        if let Some(&first) = sel.first() {
            let top = top_level(doc, page, first);
            let scope: Vec<NodeId> = sel
                .iter()
                .copied()
                .filter(|id| top_level(doc, page, *id) == top)
                .collect();
            let label = match scope.as_slice() {
                [one] => name(*one),
                many => Some(format!("{} layers", many.len())),
            };
            return (scope, label);
        }
        let res = &self.session.resolved;
        let frames: Vec<(NodeId, Rect)> = doc
            .get(page)
            .map(|p| p.children().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|id| {
                doc.get(*id)
                    .is_some_and(|n| matches!(n.kind(), ondin_core::NodeKind::Artboard { .. }))
            })
            .filter_map(|id| Some((id, res.world_bounds(id)?)))
            .collect();
        let view = self.session.camera.viewport(self.canvas_px).view;
        let in_view = |b: Rect| {
            if b.overlaps(view) {
                b.intersect(view).area()
            } else {
                0.0
            }
        };
        let pick = frames
            .iter()
            .map(|(id, b)| (*id, in_view(*b)))
            .filter(|(_, a)| *a > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
            .or_else(|| {
                crate::present::reading_order(frames.clone())
                    .first()
                    .copied()
            });
        match pick {
            Some(id) => (vec![id], name(id)),
            None => (Vec::new(), None),
        }
    }

    /// Take an Enter for the open fix card before the keymap reads it. Runs at the
    /// top of the frame.
    pub(crate) fn contrast_claim_keys(&mut self, ctx: &egui::Context) {
        if let Some(card) = self.contrast.as_mut().and_then(|c| c.fix.as_mut()) {
            card.enter |= ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        }
    }

    /// End the mode, taking back any preview its card left on the canvas.
    pub(crate) fn end_contrast(&mut self) {
        if let Some(c) = self.contrast.take()
            && c.fix.is_some_and(|f| f.previewing.is_some())
        {
            self.session.clear_gesture_preview();
        }
    }

    /// Keep the measurements current: land the worker's answer, mark what the last
    /// edit could have changed, and send it off once the document sits still. Runs
    /// before the canvas.
    pub(crate) fn contrast_tick(&mut self, ctx: &egui::Context) {
        let Some(mut c) = self.contrast.take() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if c.page != self.session.page {
            if c.fix.as_ref().is_some_and(|f| f.previewing.is_some()) {
                self.session.clear_gesture_preview();
            }
            let (scope, name) = self.contrast_target();
            c = Contrast::new(self.session.page, scope, name, c.options);
        }
        let options = self.prefs.contrast.options();
        if options != c.options {
            c.options = options;
            c.seen = None;
            c.stale.extend(c.marks.keys().copied());
            c.queue.extend(c.marks.keys().copied());
        }
        if let Some(run) = &c.job {
            match run.job.poll() {
                Ok(None) => {}
                Ok(Some(report)) => {
                    let run = c.job.take().expect("polled");
                    if run.options == c.options {
                        land(&mut c, &run.ids, report);
                    }
                }
                // A worker that died leaves its layers stale until the next edit
                // queues them; queueing them now would respawn it every frame.
                Err(()) => {
                    c.job = None;
                    log::warn!(target: "ondin", "the contrast check's worker died");
                }
            }
        }
        if c.seen.as_ref().map(|s| s.revision) != Some(self.session.revision()) {
            // No snapshot is the first look or a level change: everything is queued.
            let whole = c.seen.is_none();
            self.contrast_edited(&mut c);
            c.changed_at = now;
            if whole {
                c.measuring.get_or_insert(now);
            }
        }
        if c.job.is_none() && c.queue.is_empty() && c.stale.is_empty() {
            c.measuring = None;
        }
        if c.job.is_none() && !c.queue.is_empty() && now - c.changed_at >= SETTLE {
            // The next few in reading order, so badges arrive top down as they are
            // measured rather than all at once at the end.
            let ids: Vec<NodeId> = c
                .subjects
                .iter()
                .copied()
                .filter(|id| c.queue.contains(id))
                .take(CHUNK)
                .collect();
            for id in &ids {
                c.queue.remove(id);
            }
            // What is left queued is no longer a subject.
            if ids.is_empty() {
                c.queue.clear();
                self.contrast = Some(c);
                return;
            }
            let doc = self.session.doc.clone();
            let (page, options) = (c.page, c.options);
            let only = ids.clone();
            c.job = Some(Running {
                job: Job::spawn("contrast", ctx, move || {
                    contrast::check_with(&doc, page, options, Some(&only))
                }),
                ids,
                options,
            });
        } else if !c.queue.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
        }
        self.contrast = Some(c);
    }

    /// The document changed (or this is the first look): find the subjects again
    /// and queue the ones the change could have touched — a changed layer, any
    /// layer whose box moved, and every subject over the area either covered.
    fn contrast_edited(&self, c: &mut Contrast) {
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let page = c.page;
        c.subjects = subjects_in_scope(doc, res, page, &c.scope, c.options.non_text);
        let mut bounds = HashMap::new();
        walk(doc, page, |id| {
            if let Some(b) = res.world_bounds(id) {
                bounds.insert(id, b);
            }
        });
        let everything = match &c.seen {
            None => true,
            Some(old) => {
                old.doc.variables() != doc.variables()
                    || old.doc.styles() != doc.styles()
                    || old.doc.ground(page) != doc.ground(page)
            }
        };
        let mut dirty: Vec<Rect> = Vec::new();
        let mut changed: HashSet<NodeId> = HashSet::new();
        if let (false, Some(old)) = (everything, &c.seen) {
            walk(doc, page, |id| {
                let moved = old.bounds.get(&id) != bounds.get(&id);
                if moved || old.doc.get(id) != doc.get(id) {
                    changed.insert(id);
                    dirty.extend(old.bounds.get(&id));
                    dirty.extend(bounds.get(&id));
                }
            });
            walk(&old.doc, page, |id| {
                if doc.get(id).is_none() {
                    dirty.extend(old.bounds.get(&id));
                }
            });
        }
        let subjects: HashSet<NodeId> = c.subjects.iter().copied().collect();
        c.marks.retain(|id, _| subjects.contains(id));
        c.stale.retain(|id| subjects.contains(id));
        c.queue.retain(|id| subjects.contains(id));
        for &id in &c.subjects {
            let touched = everything
                || !c.marks.contains_key(&id) && !c.stale.contains(&id)
                || changed.contains(&id)
                || bounds
                    .get(&id)
                    .is_some_and(|b| dirty.iter().any(|d| overlaps(*b, *d)));
            if touched {
                c.queue.insert(id);
                c.stale.insert(id);
            }
        }
        c.seen = Some(Snapshot {
            revision: self.session.revision(),
            doc: doc.clone(),
            bounds,
        });
    }

    /// The badges and failure outlines, painted over the artwork in the canvas's
    /// own layer, so every floating card stays above them. A failing badge is a
    /// widget registered after the canvas, so it wins the pointer over it.
    pub(crate) fn contrast_badges(&mut self, ui: &mut egui::Ui) {
        let Some(c) = &self.contrast else { return };
        let canvas = self.canvas_rect;
        let level = c.options.level;
        let show_passing = self.prefs.contrast.show_passing;
        // A dragged layer's badge hides until release; the card's own preview is
        // not a drag.
        let dragging = self.session.has_gesture_preview()
            && !c.fix.as_ref().is_some_and(|f| f.previewing.is_some());
        let now = ui.input(|i| i.time);
        let painter = ui.painter_at(canvas);
        let mut placed: Vec<egui::Rect> = Vec::new();
        #[cfg(test)]
        let mut seen = Vec::new();
        let mut clicked = None;
        let mut spinning = false;
        for &id in &c.subjects {
            if dragging && self.moving(id) {
                continue;
            }
            let Some(mark) = c.marks.get(&id) else {
                continue;
            };
            let Some(world) = self.session.preview_world_bounds(id) else {
                continue;
            };
            let layer = self.to_screen_rect(world);
            if !layer.intersects(canvas) {
                continue;
            }
            let stale = c.stale.contains(&id);
            spinning |= stale;
            let (kind, text) = match mark {
                Mark::Measured(f) if f.passes() => {
                    if !show_passing {
                        continue;
                    }
                    (Kind::Pass, pass_text(f, level))
                }
                Mark::Measured(f) => (Kind::Fail, fail_text(f, level)),
                Mark::Skipped(why) => (Kind::Skip, format!("Not measured · {why}")),
            };
            if kind == Kind::Fail {
                painter.rect_stroke(
                    layer.expand(3.0),
                    2.0,
                    egui::Stroke::new(1.5, fade(FAIL_LINE, stale)),
                    egui::StrokeKind::Middle,
                );
            }
            let galley = painter.layout_no_wrap(
                text,
                egui::FontId::proportional(BADGE_PT),
                egui::Color32::PLACEHOLDER,
            );
            let w = 6.0 + 11.0 + 4.0 + galley.size().x + 6.0;
            let mut r = egui::Rect::from_min_size(
                egui::pos2(layer.left(), layer.top() - BADGE_LIFT - BADGE_H),
                egui::vec2(w, BADGE_H),
            );
            if r.top() < canvas.top() + 2.0 {
                r = r.translate(egui::vec2(0.0, canvas.top() + 2.0 - r.top()));
            }
            // A badge over an earlier one moves down below it.
            for _ in 0..placed.len() {
                let Some(hit) = placed.iter().find(|p| p.intersects(r)) else {
                    break;
                };
                r = r.translate(egui::vec2(0.0, hit.bottom() + 2.0 - r.top()));
            }
            placed.push(r);
            #[cfg(test)]
            seen.push((id, r));
            let mut hovered = false;
            if kind == Kind::Fail {
                let resp = ui.interact(
                    r,
                    ui.id().with(("contrast-badge", id)),
                    egui::Sense::click(),
                );
                hovered = resp.hovered();
                if resp.clicked() {
                    clicked = Some((id, r.right_top() + egui::vec2(6.0, 0.0)));
                }
            }
            paint_badge(&painter, r, kind, galley, stale, hovered, now);
        }
        if spinning {
            ui.ctx().request_repaint();
        }
        #[cfg(test)]
        if let Some(c) = &mut self.contrast {
            c.badges_seen = seen;
        }
        if let Some((id, anchor)) = clicked {
            self.session.selection.set_one(id);
            self.open_fix(id, anchor);
        }
    }

    /// Whether `id` or a layer above it is selected — what a drag moves.
    fn moving(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(i) = at {
            if self.session.selection.contains(i) {
                return true;
            }
            at = self.session.doc.get(i).and_then(|n| n.parent());
        }
        false
    }

    /// A world rectangle on screen, in points.
    fn to_screen_rect(&self, r: Rect) -> egui::Rect {
        let ppp = self.canvas_ppp.max(0.01);
        let cam = &self.session.camera;
        let a = cam.world_to_screen(Point::new(r.x0, r.y0), self.canvas_px);
        let b = cam.world_to_screen(Point::new(r.x1, r.y1), self.canvas_px);
        let o = self.canvas_rect.min;
        egui::Rect::from_min_max(
            o + egui::vec2(a.x as f32, a.y as f32) / ppp,
            o + egui::vec2(b.x as f32, b.y as f32) / ppp,
        )
    }

    /// Open the fix card on `id`'s failure, its rows worked out now.
    fn open_fix(&mut self, id: NodeId, anchor: egui::Pos2) {
        let Some(c) = &self.contrast else { return };
        let Some(Mark::Measured(f)) = c.marks.get(&id) else {
            return;
        };
        let card = fix_card(&self.session.doc, id, f.clone(), c.options.level, anchor);
        self.close_fix();
        if let Some(c) = &mut self.contrast {
            c.fix = Some(card);
        }
    }

    fn close_fix(&mut self) {
        if let Some(c) = &mut self.contrast
            && let Some(f) = c.fix.take()
            && f.previewing.is_some()
        {
            self.session.clear_gesture_preview();
        }
    }

    /// Step to the next failure (or the previous): select it, and move the camera
    /// only when it is off screen.
    pub(crate) fn contrast_step(&mut self, forward: bool) {
        let Some(c) = &mut self.contrast else { return };
        let fails = c.failures();
        if fails.is_empty() {
            return;
        }
        let n = fails.len();
        let i = match c.at.and_then(|a| fails.iter().position(|f| *f == a)) {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        let id = fails[i];
        c.at = Some(id);
        self.close_fix();
        self.session.selection.set_one(id);
        let Some(b) = self.session.preview_world_bounds(id) else {
            return;
        };
        let view = self.session.camera.viewport(self.canvas_px).view;
        if view.intersect(b) != b {
            if b.width() < view.width() * 0.9 && b.height() < view.height() * 0.9 {
                self.session.camera.center = b.center();
            } else {
                self.zoom_to(Some(b));
            }
        }
    }

    /// The bar, the level menu and the fix card: floating chrome over the canvas,
    /// drawn after it.
    pub(crate) fn contrast_chrome(&mut self, ctx: &egui::Context) {
        if self.contrast.is_none() || !self.canvas_rect.is_positive() {
            return;
        }
        self.contrast_bar(ctx);
        self.contrast_level_menu(ctx);
        self.contrast_fix_card(ctx);
        if self.contrast.as_ref().is_some_and(Contrast::owns_escape) {
            // The card and the menu own Escape, as the inspector's popovers do.
            self.popover_heard = true;
        }
    }

    fn contrast_bar(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.contrast else { return };
        let level = c.options.level;
        let fails = c.failures();
        let measured = c.measured();
        let waiting = !c.stale.is_empty() || c.job.is_some();
        let now = ctx.input(|i| i.time);
        let pulse = c
            .measuring
            .map(|since| crate::ui::pulse_alpha((now - since) as f32));
        let level_open = c.level_menu;
        if pulse.is_some() {
            ctx.request_repaint();
        }
        let at =
            c.at.and_then(|a| fails.iter().position(|f| *f == a))
                .map(|i| i + 1);
        let scope_name = c.scope_name.clone();
        // Below the top ruler, which is drawn over the canvas's top edge.
        let ruler = if self.rulers_on() {
            crate::rulers::THICKNESS
        } else {
            0.0
        };
        let mut act = None;
        egui::Area::new(egui::Id::new("contrast-bar"))
            .order(egui::Order::Middle)
            .fade_in(false)
            .pivot(egui::Align2::CENTER_TOP)
            .fixed_pos(self.canvas_rect.center_top() + egui::vec2(0.0, 12.0 + ruler))
            .show(ctx, |ui| {
                bar_frame().show(ui, |ui| {
                    ui.set_height(BAR_H - 2.0);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(theme::icon_text(icon::CIRCLE_HALF, 15.0, color::ACCENT_300));
                        match &scope_name {
                            Some(name) => {
                                if scope_chip(ui, name) {
                                    act = Some(BarAct::Retarget);
                                }
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("Contrast")
                                        .size(12.0)
                                        .color(color::TEXT),
                                );
                            }
                        }
                        // The spacing in force when an item is allocated is the gap
                        // after it. The head's ground carries its own inset, so the
                        // gaps either side of it are short by that much.
                        ui.spacing_mut().item_spacing.x = 10.0 - crate::ui::HEAD_LEAD;
                        separator(ui);
                        ui.spacing_mut().item_spacing.x = 10.0 - crate::ui::HEAD_PAD;
                        let lr = crate::ui::menu_head(
                            ui,
                            egui::RichText::new(level.label())
                                .size(12.0)
                                .color(theme::text::MUTED),
                            4.0,
                            level_open,
                        );
                        if lr.clicked() {
                            act = Some(BarAct::LevelMenu(lr.rect.left_bottom()));
                        }
                        ui.spacing_mut().item_spacing.x = 10.0;
                        separator(ui);
                        let ink = if waiting {
                            theme::text::DIM
                        } else {
                            color::TEXT
                        };
                        // A count's glyph sits 5 from its words.
                        let glyph = |ui: &mut egui::Ui, text: egui::RichText| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            ui.label(text);
                            ui.spacing_mut().item_spacing.x = 10.0;
                        };
                        if let Some(alpha) = pulse {
                            ui.label(
                                egui::RichText::new("Measuring…")
                                    .size(12.0)
                                    .color(theme::text::MUTED.gamma_multiply(alpha)),
                            );
                        } else if fails.is_empty() {
                            glyph(ui, theme::icon_text(icon::CHECK_CIRCLE, 13.0, PASS));
                            ui.label(
                                egui::RichText::new(format!("All {measured} pass"))
                                    .size(12.0)
                                    .color(ink),
                            );
                        } else {
                            glyph(ui, theme::icon_text(icon::WARNING, 13.0, FAIL_LINE));
                            ui.label(
                                egui::RichText::new(format!("{} failing", fails.len()))
                                    .size(12.0)
                                    .color(ink),
                            );
                        }
                        if pulse.is_none() && !fails.is_empty() {
                            // Room for the widest position, so stepping (which changes
                            // it) never moves the arrows under the pointer.
                            let n = fails.len();
                            let at_w = ui
                                .painter()
                                .layout_no_wrap(
                                    format!("{n} / {n}"),
                                    egui::FontId::proportional(12.0),
                                    theme::text::DIM,
                                )
                                .size()
                                .x;
                            cell(ui, at_w.ceil() + 2.0, |ui| {
                                if let Some(at) = at {
                                    ui.label(
                                        egui::RichText::new(format!("{at} / {}", fails.len()))
                                            .size(12.0)
                                            .color(theme::text::DIM),
                                    );
                                }
                            });
                            ui.spacing_mut().item_spacing.x = 2.0;
                            if crate::ui::icon_button(ui, icon::CARET_LEFT, 24.0, 13.0, false, true)
                                .on_hover_text("Previous failure")
                                .clicked()
                            {
                                act = Some(BarAct::Step(false));
                            }
                            if crate::ui::icon_button(
                                ui,
                                icon::CARET_RIGHT,
                                24.0,
                                13.0,
                                false,
                                true,
                            )
                            .on_hover_text("Next failure")
                            .clicked()
                            {
                                act = Some(BarAct::Step(true));
                            }
                        }
                        if crate::ui::icon_button(ui, icon::X, 24.0, 13.0, false, true)
                            .on_hover_text("Stop checking contrast")
                            .clicked()
                        {
                            act = Some(BarAct::Close);
                        }
                    });
                });
            });
        match act {
            None => {}
            Some(BarAct::Close) => self.end_contrast(),
            Some(BarAct::Step(forward)) => self.contrast_step(forward),
            Some(BarAct::LevelMenu(at)) => {
                if let Some(c) = &mut self.contrast {
                    c.level_menu = !c.level_menu;
                    c.menu_at = at;
                    c.menu_opening = c.level_menu;
                }
            }
            Some(BarAct::Retarget) => {
                self.close_fix();
                let (scope, name) = self.contrast_target();
                if let Some(c) = &mut self.contrast {
                    *c = Contrast::new(c.page, scope, name, c.options);
                }
            }
        }
    }

    fn contrast_level_menu(&mut self, ctx: &egui::Context) {
        let Some(head) = self
            .contrast
            .as_ref()
            .filter(|c| c.level_menu)
            .map(|c| c.menu_at)
        else {
            return;
        };
        const ITEM_H: f32 = 26.0;
        let at = head + egui::vec2(-8.0, crate::ui::MENU_GAP + 6.0);
        let level = self.prefs.contrast.options().level;
        let mut prefs = self.prefs.contrast.clone();
        let menu = egui::Area::new(egui::Id::new("contrast-level-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                crate::ui::menu_frame(5.0).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(200.0, 5.0));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for (l, label) in [
                        (Level::Aa, "AA  ·  4.5 / 3"),
                        (Level::Aaa, "AAA  ·  7 / 4.5"),
                        (Level::Apca, "APCA  ·  Lc"),
                    ] {
                        if crate::ui::menu_check(ui, label, level == l, ITEM_H) {
                            prefs.set_level(l);
                        }
                    }
                    crate::ui::menu_sep(ui);
                    if crate::ui::menu_check(ui, "Show passing", prefs.show_passing, ITEM_H) {
                        prefs.show_passing = !prefs.show_passing;
                    }
                    if crate::ui::menu_check(ui, "Non-text (3 : 1)", prefs.non_text, ITEM_H) {
                        prefs.non_text = !prefs.non_text;
                    }
                });
            })
            .response;
        if prefs != self.prefs.contrast {
            self.prefs.contrast = prefs;
            self.prefs.save();
        }
        let away = ctx.input(|i| i.pointer.any_click()) && !menu.contains_pointer();
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(c) = &mut self.contrast {
            // The level head's own click opened the menu; it is not a click away.
            if (away && !c.menu_opening) || escape {
                c.level_menu = false;
            }
            c.menu_opening = false;
        }
    }

    fn contrast_fix_card(&mut self, ctx: &egui::Context) {
        let Some(c) = &self.contrast else { return };
        let level = c.options.level;
        let Some(card) = &c.fix else { return };
        // A remeasure redoes the rows; a layer that now passes or went away closes it.
        let fresh = match c.marks.get(&card.id) {
            Some(Mark::Measured(f)) if !f.passes() => {
                (*f != card.finding).then(|| (card.id, card.anchor, f.clone(), card.previewing))
            }
            _ => {
                self.close_fix();
                return;
            }
        };
        if let Some((id, anchor, f, previewing)) = fresh {
            let mut card = fix_card(&self.session.doc, id, f, level, anchor);
            card.previewing = previewing;
            card.opening = false;
            if let Some(c) = &mut self.contrast {
                c.fix = Some(card);
            }
        }
        let Some(c) = &self.contrast else { return };
        let Some(card) = &c.fix else { return };
        let doc = &self.session.doc;
        let id = card.id;
        let name = doc.get(id).map_or("", |n| n.name()).to_string();
        let f = card.finding.clone();
        // Only a bound fill that fails is replaced, so only then does a colour
        // unbind it (`fix_transaction`).
        let bound = doc.get(id).and_then(|n| {
            let fill = n.paint().fills.iter().rev().find(|f| f.visible)?;
            let var = fill.brush.var()?;
            let uses_fill = ondin_core::TextRef::of(n.kind()).is_some_and(|t| {
                t.spans
                    .runs(t.style, t.content.len())
                    .iter()
                    .any(|(_, s)| s.color.is_none())
            });
            let drawn = match doc.resolve_brush(id, &fill.brush).as_ref() {
                Brush::Solid(c) => Some(*c),
                _ => None,
            };
            let fails = drawn
                .and_then(|c| f.score_of(level, c))
                .is_some_and(|s| s + 1e-9 < f.needs);
            (uses_fill && fails).then(|| doc.variables().get(var).map(|v| v.name.clone()))?
        });
        let (nearest, vars, opening) = (card.nearest, card.vars.clone(), card.opening);
        let anchor = clamp_card(ctx, card.anchor);
        let mut hovered: Option<Fix> = None;
        let mut chosen: Option<Fix> = None;
        let first = nearest
            .map(Fix::Colour)
            .or(vars.first().map(|v| Fix::Var(v.0)));
        let resp = egui::Area::new(egui::Id::new("contrast-fix"))
            .order(egui::Order::Foreground)
            .fade_in(false)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                crate::ui::menu_frame(0.0).show(ui, |ui| {
                    ui.set_width(CARD_W);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    card_head(ui, &name, &f, level);
                    card_colours(ui, &f, level);
                    if f.samples.len() > 1 {
                        card_modes(ui, &f, level);
                    }
                    rule(ui);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin {
                            left: 5,
                            right: 5,
                            top: 4,
                            bottom: 6,
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            eyebrow(ui, "Nearest");
                            match nearest {
                                Some(colour) => {
                                    if let Some(var) = &bound {
                                        note(ui, &format!("Unbinds {var}"));
                                    }
                                    let score = f.score_of(level, colour).unwrap_or(0.0);
                                    let row =
                                        fix_row(ui, colour, &hex(colour), &level.format(score));
                                    if row.hovered() {
                                        hovered = Some(Fix::Colour(colour));
                                    }
                                    if row.clicked() {
                                        chosen = Some(Fix::Colour(colour));
                                    }
                                }
                                None => note(ui, "No colour at this hue passes"),
                            }
                            eyebrow(ui, "Variables");
                            if vars.is_empty() {
                                note(ui, "No colour variable passes");
                            }
                            for (var, vname, swatch, score) in &vars {
                                let row = fix_row(ui, *swatch, vname, &level.format(*score));
                                if row.hovered() {
                                    hovered = Some(Fix::Var(*var));
                                }
                                if row.clicked() {
                                    chosen = Some(Fix::Var(*var));
                                }
                            }
                        });
                });
            })
            .response;

        let (escape, clicked_away) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.pointer.any_click() && !resp.contains_pointer() && !opening,
            )
        });
        let mut enter = false;
        if let Some(card) = self.contrast.as_mut().and_then(|c| c.fix.as_mut()) {
            card.opening = false;
            enter = std::mem::take(&mut card.enter);
        }
        if enter && chosen.is_none() {
            chosen = hovered.or(first);
        }
        if let Some(fix) = chosen {
            self.apply_fix(id, &f, level, fix);
            return;
        }
        if escape || clicked_away {
            // A click on another failing badge reopens the card there, in
            // `contrast_badges`, which runs before this next frame.
            self.close_fix();
            return;
        }
        self.preview_fix(id, &f, level, hovered);
    }

    /// Preview `fix` on the canvas while its row is hovered; none takes it back.
    fn preview_fix(&mut self, id: NodeId, f: &Finding, level: Level, fix: Option<Fix>) {
        let Some(c) = &mut self.contrast else { return };
        let Some(card) = &mut c.fix else { return };
        if card.previewing == fix {
            return;
        }
        card.previewing = fix;
        match fix.and_then(|fix| fix_transaction(&self.session.doc, id, f, level, fix)) {
            Some(tx) => self.session.set_preview(&tx),
            None => self.session.clear_gesture_preview(),
        }
    }

    /// Apply `fix` to the failing runs of `id` as one undo step, and close the card.
    pub(crate) fn apply_fix(&mut self, id: NodeId, f: &Finding, level: Level, fix: Fix) {
        self.close_fix();
        let Some(tx) = fix_transaction(&self.session.doc, id, f, level, fix) else {
            return;
        };
        self.session.name_step("Fix contrast", icon::CIRCLE_HALF);
        if self.session.commit(tx) {
            self.session.info("Contrast fixed");
        }
    }
}

/// Record a landed report: the layers it was sent for take their new marks, and
/// stop being stale unless an edit queued them again meanwhile.
fn land(c: &mut Contrast, ids: &[NodeId], report: Report) {
    for id in ids {
        c.marks.remove(id);
    }
    for f in report.findings {
        c.marks.insert(f.id, Mark::Measured(f));
    }
    for s in report.skipped {
        c.marks.insert(s.id, Mark::Skipped(s.why));
    }
    for id in ids {
        if !c.queue.contains(id) {
            c.stale.remove(id);
        }
    }
}

/// Every layer under `page`, the page included, in tree order.
fn walk(doc: &Document, page: NodeId, mut each: impl FnMut(NodeId)) {
    let mut stack = vec![page];
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        each(id);
        stack.extend(n.children().iter().rev().copied());
    }
}

/// Whether two boxes touch, a degenerate (line) box included.
fn overlaps(a: Rect, b: Rect) -> bool {
    a.x0 <= b.x1 && b.x0 <= a.x1 && a.y0 <= b.y1 && b.y0 <= a.y1
}

/// The page's top-level layer at or above `id`.
fn top_level(doc: &Document, page: NodeId, id: NodeId) -> NodeId {
    let mut at = id;
    while let Some(p) = doc.get(at).and_then(|n| n.parent()) {
        if p == page {
            break;
        }
        at = p;
    }
    at
}

/// The check's subjects at or under a layer of `scope` (every one, for an empty
/// scope), in reading order: the page's top-level layers as a page is read, and
/// each one's subjects the same way within it.
pub(crate) fn subjects_in_scope(
    doc: &Document,
    res: &ondin_core::Resolved,
    page: NodeId,
    scope: &[NodeId],
    non_text: bool,
) -> Vec<NodeId> {
    let inside = |id: NodeId| {
        let mut at = Some(id);
        while let Some(i) = at {
            if scope.contains(&i) {
                return true;
            }
            at = doc.get(i).and_then(|n| n.parent());
        }
        false
    };
    let top = |id: NodeId| top_level(doc, page, id);
    let mut groups: Vec<(NodeId, Vec<(NodeId, Rect)>)> = Vec::new();
    for id in contrast::subjects(doc, res, page, non_text) {
        if !scope.is_empty() && !inside(id) {
            continue;
        }
        let b = res.world_bounds(id).unwrap_or_default();
        let t = top(id);
        match groups.iter_mut().find(|(g, _)| *g == t) {
            Some((_, members)) => members.push((id, b)),
            None => groups.push((t, vec![(id, b)])),
        }
    }
    let tops: Vec<(NodeId, Rect)> = groups
        .iter()
        .map(|(t, _)| (*t, res.world_bounds(*t).unwrap_or_default()))
        .collect();
    crate::present::reading_order(tops)
        .into_iter()
        .flat_map(|t| {
            let members = groups
                .iter()
                .find(|(g, _)| *g == t)
                .map(|(_, m)| m.clone())
                .unwrap_or_default();
            crate::present::reading_order(members)
        })
        .collect()
}

/// The fix card's rows for a failing finding, worked out once.
fn fix_card(doc: &Document, id: NodeId, f: Finding, level: Level, anchor: egui::Pos2) -> FixCard {
    let worst = f.worst().cloned();
    let ink = worst.as_ref().map_or(Color::BLACK, |s| s.ink);
    let nearest = contrast::nearest(&f, level, ink);
    let vars = contrast::passing_variables(doc, &f, level, ink)
        .into_iter()
        .take(MAX_VARS)
        .filter_map(|(v, score)| {
            let var = doc.variables().get(v)?;
            let swatch = match doc
                .variables()
                .resolve_opt(v, worst.as_ref().and_then(|s| s.mode))
            {
                Some(ondin_core::Value::Color(c)) => c,
                _ => return None,
            };
            Some((v, var.name.clone(), swatch, score))
        })
        .collect();
    FixCard {
        id,
        anchor,
        finding: f,
        nearest,
        vars,
        previewing: None,
        opening: true,
        enter: false,
    }
}

/// The edit `fix` makes to `id`: the runs and paints whose colour fails `f` take
/// the new colour, or the variable. A run's own colour is replaced in the spans; a
/// run drawn in the fill changes the fill. Binding a variable goes through the
/// fill (a run's colour cannot be bound), so a failing run's own colour is
/// cleared onto it, and a passing run that drew in the fill keeps its colour.
/// `None` when nothing fails.
pub(crate) fn fix_transaction(
    doc: &Document,
    id: NodeId,
    f: &Finding,
    level: Level,
    fix: Fix,
) -> Option<Transaction> {
    let node = doc.get(id)?;
    let fails = |c: Color| f.score_of(level, c).is_some_and(|s| s + 1e-9 < f.needs);
    let paint = doc.resolved_paint(id, node.paint());
    let solid = |b: &Brush| match b {
        Brush::Solid(c) => Some(*c),
        _ => None,
    };
    let replace = |old: &BrushSource| -> BrushSource {
        match fix {
            Fix::Colour(c) => BrushSource::Literal(Brush::Solid(c)),
            Fix::Var(var) => BrushSource::Bound {
                var,
                alpha: match old {
                    BrushSource::Literal(Brush::Solid(c)) => c.components[3],
                    BrushSource::Bound { alpha, .. } => *alpha,
                    BrushSource::Literal(_) => 1.0,
                },
            },
        }
    };
    let mut ops = Vec::new();
    if let Some(text) = ondin_core::TextRef::of(node.kind()) {
        let top = node.paint().fills.iter().rposition(|x| x.visible);
        let top_colour = top.and_then(|i| solid(paint.fills.get(i)?.brush.drawn()));
        let runs = text.spans.runs(text.style, text.content.len());
        let mut spans = text.spans.clone();
        let (mut spans_changed, mut onto_fill) = (false, false);
        let fill_fails =
            top_colour.is_some_and(fails) && runs.iter().any(|(_, s)| s.color.is_none());
        for (range, style) in &runs {
            if let Some(c) = style.color
                && fails(c)
            {
                let attr = match fix {
                    Fix::Colour(n) => CharAttr::Color(Some(n)),
                    Fix::Var(_) => {
                        onto_fill = true;
                        CharAttr::Color(None)
                    }
                };
                spans.set(range.clone(), attr, text.style);
                spans_changed = true;
            }
        }
        if onto_fill && !fill_fails {
            // The fill is about to change under the runs that drew in it and passed.
            for (range, style) in &runs {
                if style.color.is_none()
                    && let Some(c) = top_colour
                {
                    spans.set(range.clone(), CharAttr::Color(Some(c)), text.style);
                }
            }
        }
        if spans_changed {
            ops.push(Operation::SetTextSpans { id, spans });
        }
        if fill_fails || onto_fill {
            let mut fills = node.paint().fills.clone();
            match top {
                Some(i) => {
                    let b = replace(&fills[i].brush);
                    fills[i].brush = b;
                }
                None => {
                    fills = ondin_core::keyed_by_position([Fill {
                        brush: replace(&BrushSource::Literal(Brush::Solid(Color::BLACK))),
                        visible: true,
                    }]);
                }
            }
            ops.push(Operation::SetFills { id, fills });
        }
    } else {
        let mut fills = node.paint().fills.clone();
        let mut fills_changed = false;
        for (i, fl) in fills.iter_mut().enumerate() {
            let drawn = paint.fills.get(i).and_then(|p| solid(p.brush.drawn()));
            if fl.visible && drawn.is_some_and(fails) {
                let b = replace(&fl.brush);
                fl.brush = b;
                fills_changed = true;
            }
        }
        let mut strokes = node.paint().strokes.clone();
        let mut strokes_changed = false;
        for (i, st) in strokes.iter_mut().enumerate() {
            let drawn = paint.strokes.get(i).and_then(|p| solid(p.brush.drawn()));
            if st.visible && drawn.is_some_and(fails) {
                let b = replace(&st.brush);
                st.brush = b;
                strokes_changed = true;
            }
        }
        if fills_changed {
            ops.push(Operation::SetFills { id, fills });
        }
        if strokes_changed {
            ops.push(Operation::SetStrokes { id, strokes });
        }
    }
    (!ops.is_empty()).then_some(Transaction(ops))
}

/// A passing chip's words: the score alone, and `large` when only the text's
/// size makes it pass.
fn pass_text(f: &Finding, level: Level) -> String {
    let score = level.format(f.ratio).trim_end_matches(" : 1").to_string();
    if f.large && f.ratio + 1e-9 < level.body() {
        format!("{score} · large")
    } else {
        score
    }
}

/// A failing badge's words: the score, the mode it failed in when there are
/// several, and what it needs.
fn fail_text(f: &Finding, level: Level) -> String {
    let mode = f
        .mode
        .as_ref()
        .map(|m| format!("{m} · "))
        .unwrap_or_default();
    format!(
        "{} · {mode}needs {}",
        level.format(f.ratio),
        level.format_needs(f.needs)
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Pass,
    Fail,
    Skip,
}

/// `c`, dimmed while its badge waits on a remeasure.
fn fade(c: egui::Color32, stale: bool) -> egui::Color32 {
    if stale { c.gamma_multiply(0.45) } else { c }
}

fn paint_badge(
    painter: &egui::Painter,
    r: egui::Rect,
    kind: Kind,
    galley: std::sync::Arc<egui::Galley>,
    stale: bool,
    hovered: bool,
    now: f64,
) {
    let radius = egui::CornerRadius::same(4);
    let (ground, ink, glyph) = match kind {
        Kind::Fail if hovered => (FAIL_HOVER, egui::Color32::WHITE, icon::WARNING),
        Kind::Fail => (FAIL, egui::Color32::WHITE, icon::WARNING),
        Kind::Pass => (CHIP, egui::Color32::from_white_alpha(209), icon::CHECK),
        Kind::Skip => (CHIP, egui::Color32::from_white_alpha(178), icon::PROHIBIT),
    };
    if !stale {
        painter.add(
            egui::epaint::Shadow {
                offset: [0, 2],
                blur: 6,
                spread: 0,
                color: egui::Color32::from_black_alpha(76),
            }
            .as_shape(r, radius),
        );
    }
    painter.rect_filled(r, radius, fade(ground, stale));
    if hovered {
        // A ring as well as the lighter ground: the badge is the one click target
        // on the canvas, and the arrow cursor does not say so.
        painter.rect_stroke(
            r.expand(1.5),
            5.0,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(140)),
            egui::StrokeKind::Middle,
        );
    }
    if kind == Kind::Skip {
        let stroke = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(89));
        let c = [
            r.left_top(),
            r.right_top(),
            r.right_bottom(),
            r.left_bottom(),
        ];
        for i in 0..4 {
            painter.extend(egui::Shape::dashed_line(
                &[c[i], c[(i + 1) % 4]],
                stroke,
                3.0,
                2.0,
            ));
        }
    }
    let ink = fade(ink, stale);
    let glyph_at = egui::pos2(r.left() + 6.0 + 5.5, r.center().y);
    if stale {
        // A spinner where the glyph was.
        let g = painter.layout_no_wrap(icon::CIRCLE_NOTCH.into(), theme::icon_font(11.0), ink);
        let angle = (now * 6.0 % std::f64::consts::TAU) as f32;
        let half = g.size() / 2.0;
        let rot = egui::emath::Rot2::from_angle(angle);
        let pos = glyph_at - rot * half;
        painter.add(egui::epaint::TextShape::new(pos, g, ink).with_angle(angle));
    } else {
        painter.text(
            glyph_at,
            egui::Align2::CENTER_CENTER,
            glyph,
            theme::icon_font(11.0),
            ink,
        );
    }
    let text_at = egui::pos2(
        r.left() + 6.0 + 11.0 + 4.0,
        r.center().y - galley.size().y / 2.0,
    );
    painter.galley(text_at, galley, ink);
}

const BAR_H: f32 = 34.0;

fn bar_frame() -> egui::Frame {
    crate::ui::menu_frame(0.0)
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin {
            left: 12,
            right: 6,
            top: 1,
            bottom: 1,
        })
}

/// A hairline between the bar's groups, with 3 points of its own on each side
/// beyond the row's spacing.
fn separator(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(7.0, 16.0), egui::Sense::hover());
    let line = egui::Rect::from_center_size(r.center(), egui::vec2(1.0, 16.0));
    ui.painter().rect_filled(line, 0.0, color::text_a(30));
}

/// The bar's scope chip: a pill naming what is checked, clear of the bar's edges.
/// A click checks the selection, or the frame in view, instead; answers whether
/// it was clicked.
fn scope_chip(ui: &mut egui::Ui, name: &str) -> bool {
    const H: f32 = 20.0;
    const PAD: f32 = 8.0;
    const GAP: f32 = 5.0;
    let font = egui::FontId::proportional(12.0);
    let galley = ui
        .painter()
        .layout_no_wrap(name.to_string(), font, color::TEXT);
    let w = PAD + 12.0 + GAP + galley.size().x + PAD + 2.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, H), egui::Sense::click());
    let p = ui.painter();
    let ground = if resp.hovered() {
        color::text_a(32)
    } else {
        color::text_a(20)
    };
    p.rect_filled(rect, H / 2.0, ground);
    p.text(
        egui::pos2(rect.left() + PAD + 6.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::FRAME_CORNERS,
        theme::icon_font(12.0),
        color::ACCENT_400,
    );
    p.galley(
        egui::pos2(
            rect.left() + PAD + 12.0 + GAP,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color::TEXT,
    );
    resp.on_hover_text("Check the selection, or the frame in view, instead")
        .clicked()
}

/// A left-aligned cell of fixed width in the bar's row.
fn cell(ui: &mut egui::Ui, w: f32, add: impl FnOnce(&mut egui::Ui)) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, BAR_H - 2.0), egui::Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    add(&mut child);
}

/// What the bar asked for this frame, acted on after it is drawn.
enum BarAct {
    Close,
    Step(bool),
    LevelMenu(egui::Pos2),
    Retarget,
}

/// The card on screen whole: shifted left and up when its anchor is near the
/// window's edge.
fn clamp_card(ctx: &egui::Context, at: egui::Pos2) -> egui::Pos2 {
    let screen = ctx.input(|i| i.content_rect());
    egui::pos2(
        at.x.min(screen.right() - CARD_W - 8.0)
            .max(screen.left() + 8.0),
        at.y.min(screen.bottom() - 320.0).max(screen.top() + 8.0),
    )
}

fn card_head(ui: &mut egui::Ui, name: &str, f: &Finding, level: Level) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 11,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let glyph = if f.non_text {
                    icon::SHAPES
                } else {
                    icon::TEXT_T
                };
                ui.label(theme::icon_text(glyph, 15.0, color::ACCENT_300));
                ui.label(egui::RichText::new(name).size(12.5).color(color::TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let what = match () {
                        _ if f.non_text => "non-text",
                        _ if f.large => "large text",
                        _ => "body text",
                    };
                    ui.label(
                        egui::RichText::new(format!("{what} · {}", level.label()))
                            .size(10.5)
                            .color(theme::text::DIM),
                    );
                });
            });
        });
    rule(ui);
}

fn card_colours(ui: &mut egui::Ui, f: &Finding, level: Level) {
    let Some(w) = f.worst() else { return };
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                split_swatch(ui, w.ink, w.backdrop, egui::vec2(22.0, 22.0));
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        egui::RichText::new(format!("{} on {}", hex(w.ink), hex(w.backdrop)))
                            .size(10.5)
                            .color(color::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(if f.non_text {
                            "mark on backdrop"
                        } else {
                            "text on backdrop"
                        })
                        .size(10.5)
                        .color(theme::text::DIM),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(level.format(f.ratio))
                            .size(15.0)
                            .strong()
                            .color(FAIL_LINE),
                    );
                });
            });
        });
}

/// One row per mode: its colours and score, a fail marked by glyph and colour.
fn card_modes(ui: &mut egui::Ui, f: &Finding, level: Level) {
    rule(ui);
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(5, 5))
        .show(ui, |ui| {
            for s in &f.samples {
                let pass = s.score + 1e-9 >= f.needs;
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 28.0),
                    egui::Sense::hover(),
                );
                let mut child = ui
                    .new_child(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(8.0, 0.0))));
                child.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    split_swatch(ui, s.ink, s.backdrop, egui::vec2(12.0, 16.0));
                    ui.label(
                        egui::RichText::new(s.name.as_deref().unwrap_or("Default"))
                            .size(11.5)
                            .color(color::TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ink = if pass { theme::text::MUTED } else { FAIL_LINE };
                        ui.label(
                            egui::RichText::new(level.format(s.score))
                                .size(11.5)
                                .color(ink),
                        );
                        let glyph = if pass { icon::CHECK } else { icon::WARNING };
                        ui.label(theme::icon_text(glyph, 12.0, ink));
                    });
                });
            }
        });
}

/// Two colours side by side, the text's then the backdrop's.
fn split_swatch(ui: &mut egui::Ui, ink: Color, backdrop: Color, half: egui::Vec2) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(half.x * 2.0, half.y), egui::Sense::hover());
    let (l, rr) = r.split_left_right_at_fraction(0.5);
    let p = ui.painter();
    p.rect_filled(
        l,
        egui::CornerRadius {
            nw: 4,
            sw: 4,
            ne: 0,
            se: 0,
        },
        color32(ink),
    );
    p.rect_filled(
        rr,
        egui::CornerRadius {
            nw: 0,
            sw: 0,
            ne: 4,
            se: 4,
        },
        color32(backdrop),
    );
    p.rect_stroke(
        r,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30)),
        egui::StrokeKind::Inside,
    );
}

/// One fix: its swatch, its name and the score it would get.
fn fix_row(ui: &mut egui::Ui, swatch: Color, name: &str, score: &str) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 4.0, color::text_a(20));
    }
    let sw = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 8.0 + 7.0, rect.center().y),
        egui::vec2(14.0, 14.0),
    );
    p.rect_filled(sw, 4.0, color32(swatch));
    p.rect_stroke(
        sw,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(36)),
        egui::StrokeKind::Inside,
    );
    p.text(
        egui::pos2(sw.right() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(11.5),
        color::TEXT,
    );
    p.text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        score,
        egui::FontId::proportional(11.5),
        theme::text::MUTED,
    );
    resp
}

fn eyebrow(ui: &mut egui::Ui, label: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(crate::ui::eyebrow(label));
    });
    ui.add_space(3.0);
}

fn note(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(text).size(11.0).color(theme::text::DIM));
    });
    ui.add_space(4.0);
}

fn rule(ui: &mut egui::Ui) {
    let (r, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 0.0, color::DIVIDER);
}

fn color32(c: Color) -> egui::Color32 {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

/// `RRGGBB`, as the card names a colour.
fn hex(c: Color) -> String {
    let [r, g, b, _] = c.to_rgba8().to_u8_array();
    format!("{r:02X}{g:02X}{b:02X}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{NodeKind, TextSizing, TextStyle};

    /// One whole app frame at a fixed window size, the clock a twentieth of a
    /// second on (short enough for a press and release to be a click), with
    /// `events`.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let time = ctx.input(|i| i.time) + 0.05;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    /// Pump frames until every queued layer is measured, waiting on the worker
    /// rather than sleeping.
    fn settle(ctx: &egui::Context, app: &mut OndinApp) {
        for _ in 0..40 {
            frame(ctx, app, Vec::new());
            let Some(c) = &mut app.contrast else { return };
            if let Some(run) = c.job.take() {
                let report = run.job.wait().expect("the worker answers");
                land(c, &run.ids, report);
                continue;
            }
            if c.queue.is_empty() && c.seen.is_some() {
                frame(ctx, app, Vec::new());
                return;
            }
        }
        panic!("the check never settled");
    }

    fn solid(r: u8, g: u8, b: u8) -> Vec<ondin_core::Keyed<Fill>> {
        ondin_core::keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(r, g, b, 255)).into(),
            visible: true,
        }])
    }

    fn text_op(id: NodeId, parent: NodeId, at: (f64, f64), name: &str) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Text {
                content: "Billed yearly".into(),
                style: Box::new(TextStyle::default()),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate(at)),
            name: Some(name.into()),
        }
    }

    struct Fixture {
        app: OndinApp,
        /// A white frame holding `grey` (failing) and `black` (passing).
        card: NodeId,
        grey: NodeId,
        black: NodeId,
        /// A second white frame, far to the right, holding `other`, which fails.
        other: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> Fixture {
        let mut app = OndinApp::headless(ctx);
        let page = app.session.ids.mint();
        app.session.adopt_document(Document::new(page), None);
        let ids = &mut app.session.ids;
        let (card, far, grey, black, other) =
            (ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let frame_op = |id, x: f64, name: &str| Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 200.0),
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: Some(name.into()),
        };
        assert!(app.session.commit(Transaction(vec![
            frame_op(card, 0.0, "Card"),
            Operation::SetFills {
                id: card,
                fills: solid(255, 255, 255),
            },
            frame_op(far, 2000.0, "Far"),
            Operation::SetFills {
                id: far,
                fills: solid(255, 255, 255),
            },
            text_op(grey, card, (20.0, 120.0), "Caption"),
            Operation::SetFills {
                id: grey,
                fills: solid(170, 170, 170),
            },
            text_op(black, card, (20.0, 20.0), "Title"),
            Operation::SetFills {
                id: black,
                fills: solid(0, 0, 0),
            },
            text_op(other, far, (20.0, 20.0), "Note"),
            Operation::SetFills {
                id: other,
                fills: solid(190, 190, 190),
            },
        ])));
        app.zoom_to(Some(Rect::new(-20.0, -20.0, 320.0, 220.0)));
        frame(ctx, &mut app, Vec::new());
        Fixture {
            app,
            card,
            grey,
            black,
            other,
        }
    }

    fn passes(app: &OndinApp, id: NodeId) -> Option<bool> {
        match app.contrast.as_ref()?.marks.get(&id)? {
            Mark::Measured(f) => Some(f.passes()),
            Mark::Skipped(_) => None,
        }
    }

    /// The bar says "Measuring…" through a whole run (the first look, a level
    /// change) and not through an edit's remeasure.
    /// flip: `contrast_tick` starting a run on any edit (`whole` always true) →
    /// fails at "an edit's remeasure is not a run".
    #[test]
    fn measuring_spans_a_whole_run_only() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.toggle_contrast();
        frame(&ctx, &mut f.app, Vec::new());
        let measuring = |app: &OndinApp| app.contrast.as_ref().unwrap().measuring.is_some();
        assert!(measuring(&f.app), "the first look is a run");
        settle(&ctx, &mut f.app);
        assert!(!measuring(&f.app), "the run ends once all is measured");
        assert!(f.app.session.commit(Transaction(vec![Operation::SetFills {
            id: f.grey,
            fills: solid(40, 40, 40),
        }])));
        frame(&ctx, &mut f.app, Vec::new());
        assert!(
            f.app.contrast.as_ref().unwrap().stale.contains(&f.grey),
            "fixture: the caption waits on a remeasure"
        );
        assert!(!measuring(&f.app), "an edit's remeasure is not a run");
        settle(&ctx, &mut f.app);
        f.app.prefs.contrast.set_level(Level::Aaa);
        frame(&ctx, &mut f.app, Vec::new());
        assert!(measuring(&f.app), "a level change is a run");
    }

    /// With nothing selected the mode measures the frame in view, lists its
    /// failures in reading order, and edits nothing; the far frame is left out.
    #[test]
    fn the_check_lists_the_frame_in_view_s_failures_in_reading_order() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let revision = f.app.session.revision();
        f.app.dispatch(&ctx, crate::input::Action::CheckContrast);
        settle(&ctx, &mut f.app);
        let c = f.app.contrast.as_ref().expect("on");
        assert_eq!(c.scope, [f.card]);
        assert_eq!(c.subjects, [f.black, f.grey], "reading order");
        assert_eq!(c.failures(), [f.grey]);
        assert_eq!(c.measured(), 2);
        assert!(c.stale.is_empty());
        assert_eq!(passes(&f.app, f.black), Some(true));
        assert_eq!(f.app.session.revision(), revision, "no edit");
        f.app.dispatch(&ctx, crate::input::Action::CheckContrast);
        assert!(f.app.contrast.is_none(), "the same action ends it");
    }

    /// The selection the mode starts with is its scope, and selecting something
    /// else afterwards leaves the scope alone.
    #[test]
    fn the_starting_selection_is_the_scope() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set_one(f.card);
        f.app.toggle_contrast();
        settle(&ctx, &mut f.app);
        let c = f.app.contrast.as_ref().unwrap();
        assert_eq!(c.subjects, [f.black, f.grey]);
        assert_eq!(c.scope_name.as_deref(), Some("Card"));
        f.app.session.selection.set_one(f.other);
        settle(&ctx, &mut f.app);
        assert_eq!(f.app.contrast.as_ref().unwrap().subjects, [f.black, f.grey]);
    }

    /// A selection across frames is cut to its first layer's frame, and with
    /// nothing selected the frame filling the view is checked.
    ///
    /// flip: `contrast_target` keeping the whole selection → fails at "cut to
    /// one frame".
    #[test]
    fn the_target_is_one_frame_at_most() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.session.selection.set(vec![f.grey, f.other]);
        assert_eq!(f.app.contrast_target().0, [f.grey], "cut to one frame");
        f.app.session.selection.clear();
        f.app.zoom_to(Some(Rect::new(1980.0, -20.0, 2320.0, 220.0)));
        let (scope, name) = f.app.contrast_target();
        assert_eq!(name.as_deref(), Some("Far"));
        assert_eq!(scope.len(), 1);
        assert!(
            f.app
                .session
                .doc
                .get(scope[0])
                .is_some_and(|n| n.name() == "Far")
        );
    }

    /// An edit queues the layers it could have changed and no others: darkening
    /// the caption remeasures it, and the title above it keeps its mark.
    ///
    /// flip: `contrast_edited` treating every edit as touching everything
    /// (`everything` always true) → fails at "the title is not touched".
    #[test]
    fn an_edit_remeasures_what_it_touched() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.toggle_contrast();
        settle(&ctx, &mut f.app);
        assert_eq!(passes(&f.app, f.grey), Some(false), "fixture: fails");
        assert!(f.app.session.commit(Transaction(vec![Operation::SetFills {
            id: f.grey,
            fills: solid(40, 40, 40),
        }])));
        frame(&ctx, &mut f.app, Vec::new());
        let c = f.app.contrast.as_ref().unwrap();
        assert!(
            c.stale.contains(&f.grey),
            "the caption waits on a remeasure"
        );
        assert!(!c.stale.contains(&f.black), "the title is not touched");
        settle(&ctx, &mut f.app);
        assert_eq!(passes(&f.app, f.grey), Some(true));
    }

    /// The card's nearest colour makes the layer pass, as one undo step.
    #[test]
    fn the_nearest_fix_passes_in_one_step() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.toggle_contrast();
        settle(&ctx, &mut f.app);
        f.app.open_fix(f.grey, egui::pos2(400.0, 100.0));
        let card = f.app.contrast.as_ref().unwrap().fix.as_ref().expect("open");
        let colour = card.nearest.expect("a darker grey passes");
        let finding = card.finding.clone();
        let depth = f.app.session.history.undo_depth();
        f.app
            .apply_fix(f.grey, &finding, Level::Aa, Fix::Colour(colour));
        assert_eq!(f.app.session.history.undo_depth(), depth + 1);
        assert!(
            f.app.contrast.as_ref().unwrap().fix.is_none(),
            "the card closes"
        );
        settle(&ctx, &mut f.app);
        assert_eq!(passes(&f.app, f.grey), Some(true));
        f.app.undo();
        settle(&ctx, &mut f.app);
        assert_eq!(passes(&f.app, f.grey), Some(false), "undone");
    }

    /// A click on a failing badge opens the card on that layer and selects it,
    /// and the press never reaches the canvas under it. The control: the same
    /// press with the mode off changes the selection.
    ///
    /// flip: `contrast_badges` called before `canvas_ui` → fails at "the card is
    /// open".
    #[test]
    fn a_click_on_a_failing_badge_opens_the_card() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.toggle_contrast();
        settle(&ctx, &mut f.app);
        let (_, r) = *f
            .app
            .contrast
            .as_ref()
            .unwrap()
            .badges_seen
            .iter()
            .find(|(id, _)| *id == f.grey)
            .expect("the caption has a badge");
        let p = r.center();
        let revision = f.app.session.revision();
        frame(&ctx, &mut f.app, vec![egui::Event::PointerMoved(p)]);
        frame(&ctx, &mut f.app, vec![button(p, true)]);
        frame(&ctx, &mut f.app, vec![button(p, false)]);
        let c = f.app.contrast.as_ref().unwrap();
        assert_eq!(
            c.fix.as_ref().map(|f| f.id),
            Some(f.grey),
            "the card is open"
        );
        assert_eq!(f.app.session.selection.ids(), &[f.grey]);
        assert_eq!(f.app.session.revision(), revision, "nothing moved");

        f.app.end_contrast();
        f.app.session.selection.set_one(f.black);
        frame(&ctx, &mut f.app, Vec::new());
        frame(&ctx, &mut f.app, vec![button(p, true)]);
        frame(&ctx, &mut f.app, vec![button(p, false)]);
        assert_ne!(
            f.app.session.selection.ids(),
            &[f.black],
            "control: with the mode off the canvas takes the press"
        );
    }

    /// Enter with the card open applies its first fix, and the keymap never sees
    /// it: the selected text layer is not opened for editing.
    ///
    /// flip: `contrast_claim_keys` not consuming the key (`input` for
    /// `input_mut`) → fails at "no text editing".
    #[test]
    fn enter_applies_the_card_s_first_fix() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.toggle_contrast();
        settle(&ctx, &mut f.app);
        f.app.session.selection.set_one(f.grey);
        f.app.open_fix(f.grey, egui::pos2(400.0, 100.0));
        frame(&ctx, &mut f.app, Vec::new());
        let depth = f.app.session.history.undo_depth();
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut f.app, vec![enter(true)]);
        frame(&ctx, &mut f.app, vec![enter(false)]);
        assert!(f.app.text.is_none(), "no text editing");
        assert_eq!(f.app.session.history.undo_depth(), depth + 1, "fixed");
        settle(&ctx, &mut f.app);
        assert_eq!(passes(&f.app, f.grey), Some(true));
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// A variable fix on a run with its own colour clears the run onto the fill
    /// and binds the fill, since a run's colour cannot be bound; a passing run
    /// that drew in the fill keeps its look.
    #[test]
    fn a_variable_fix_binds_through_the_fill() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let (var, mode) = (f.app.session.ids.mint_var(), f.app.session.ids.mint_mode());
        let grey = Color::from_rgba8(170, 170, 170, 255);
        let mut spans = ondin_core::CharSpans::default();
        spans.set(0..6, CharAttr::Color(Some(grey)), &TextStyle::default());
        assert!(f.app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: ondin_core::Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: ondin_core::Variable {
                    id: var,
                    name: "text/primary".into(),
                    kind: ondin_core::VarKind::Color,
                    cells: [(mode, ondin_core::Cell::Color(Color::BLACK))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFills {
                id: f.black,
                fills: solid(0, 0, 0),
            },
            Operation::SetTextSpans { id: f.black, spans },
        ])));
        let doc = &f.app.session.doc;
        let finding = Finding {
            id: f.black,
            trail: vec![],
            ratio: 2.3,
            needs: 4.5,
            large: false,
            non_text: false,
            mode: None,
            samples: vec![contrast::Sample {
                mode: None,
                name: None,
                score: 2.3,
                ink: grey,
                opacity: 1.0,
                backdrop: Color::WHITE,
            }],
        };
        let tx = fix_transaction(doc, f.black, &finding, Level::Aa, Fix::Var(var)).expect("a fix");
        let mut after = doc.clone();
        after.apply(&tx).unwrap();
        let n = after.get(f.black).unwrap();
        let t = ondin_core::TextRef::of(n.kind()).unwrap();
        let runs = t.spans.runs(t.style, t.content.len());
        assert_eq!(runs[0].1.color, None, "the failing run falls onto the fill");
        assert_eq!(
            runs[1].1.color,
            Some(Color::from_rgba8(0, 0, 0, 255)),
            "the passing run keeps black"
        );
        assert_eq!(
            n.paint().fills[0].brush.var(),
            Some(var),
            "the fill is bound"
        );
    }

    /// Opening a document turns the mode off.
    #[test]
    fn opening_a_document_ends_the_check() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.toggle_contrast();
        let page = f.app.session.ids.mint();
        f.app.adopt_opened(
            &std::env::temp_dir().join("ondin-contrast-open.ondin"),
            Document::new(page),
        );
        assert!(f.app.contrast.is_none());
    }
}
