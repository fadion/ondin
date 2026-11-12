//! The Clean up card: the path bar's wand opens it beside the bar, as *Offset*
//! opens its card. Four rows, each shown on the canvas while its button is hovered
//! or focused or its field scrubbed, with the points it would take away marked, and
//! each committed by its button as an undo step of its own: *Simplify* within an
//! accuracy, *Remove redundant points*, *Add points* and *Smooth selected*.
//!
//! Simplify's refit runs on a worker ([`Job`]): the canvas shows the newest refit to
//! land, and the button waits for the accuracy the field shows. The card opens with
//! that button focused, so the wand and then `Enter` simplifies in one go.

use crate::app::OndinApp;
use crate::job::Job;
use crate::preview::PointRef;
use crate::theme::{self, icon};
use crate::tools;
use crate::ui::{self, FieldButton, Prefix, Scrub, Suffix};
use eframe::egui;
use ondin_core::kurbo::{BezPath, Point};
use ondin_core::{GeometryPatch, NodeId, NodeKind, Operation, Transaction, build};
use std::sync::Arc;

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;

/// The most pieces *Add points* cuts a segment into.
const MAX_PIECES: f64 = 16.0;

/// How long, in seconds, Simplify's accuracy may wait on its refit before the
/// caption says *Fitting…*; until then it keeps the last count, so a fit of a few
/// milliseconds never flashes. The size preview waits as long before it fades its
/// tiles and spins.
pub(crate) const FIT_PATIENCE: f64 = 0.25;

/// The card's numbers, kept while it is closed so it reopens on the last ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CleanupNumbers {
    /// How far *Simplify* may move the outline, in the layer's units.
    pub(crate) accuracy: f64,
    /// How many pieces *Add points* cuts each segment into.
    pub(crate) pieces: f64,
    /// How far *Smooth selected* takes the handles, in percent.
    pub(crate) amount: f64,
}

impl Default for CleanupNumbers {
    fn default() -> Self {
        Self {
            accuracy: build::SIMPLIFY_ACCURACY,
            pieces: 2.0,
            amount: 50.0,
        }
    }
}

/// One of the card's rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Simplify,
    Remove,
    Add,
    Smooth,
}

impl Row {
    /// The undo step a row's commit makes.
    fn step(self) -> (&'static str, &'static str) {
        match self {
            Row::Simplify => ("Simplify", icon::MAGIC_WAND),
            Row::Remove => ("Remove points", icon::TRASH),
            Row::Add => ("Add points", icon::PLUS),
            Row::Smooth => ("Smooth", icon::NODE_SMOOTH),
        }
    }
}

/// The open card: its path, read once per change, and Simplify's refits of it.
pub(crate) struct CleanupCard {
    id: NodeId,
    /// The path as stored when the card last read it. Any other state of it (an
    /// undo, a drag, one of the card's own commits) starts the card over.
    basis: NodeKind,
    /// What Simplify refits: the stored path with its radii applied.
    drawn: Arc<BezPath>,
    /// The newest refit to land, and the accuracy it was made at.
    fitted: Option<(f64, BezPath)>,
    /// The refit on the worker, and its accuracy. One at a time, as the Offset card's.
    fitting: Option<(f64, Job<BezPath>)>,
    /// Since when, in `egui` input time, the accuracy in the field has had no
    /// refit landed ([`FIT_PATIENCE`]).
    unanswered: Option<f64>,
    /// How many anchors *Remove redundant points* would take out.
    redundant: usize,
    /// The row the canvas shows, and what it was built from.
    shown: Option<(Row, ShownKey)>,
    /// The points the shown row would take away, in world space, which the canvas
    /// marks.
    pub(crate) dropping: Vec<Point>,
    /// Focus Simplify's button on the first frame.
    focus_apply: bool,
}

/// What a shown row was built from: the numbers, the point selection and the
/// accuracy of the refit it used.
type ShownKey = (CleanupNumbers, Vec<PointRef>, Option<f64>);

impl CleanupCard {
    /// Take in a refit that has landed and set the worker on `accuracy` if it is not
    /// already done; whether anything landed.
    fn pump(&mut self, ctx: &egui::Context, accuracy: f64) -> bool {
        let mut fresh = false;
        if let Some((a, job)) = &self.fitting {
            match job.poll() {
                Ok(Some(path)) => {
                    self.fitted = Some((*a, path));
                    self.fitting = None;
                    fresh = true;
                }
                Ok(None) => {}
                Err(()) => self.fitting = None,
            }
        }
        if self.fitting.is_none() && !self.fitted_at(accuracy) {
            let drawn = Arc::clone(&self.drawn);
            let job = Job::spawn("ondin-simplify", ctx, move || {
                build::simplify_outline_within(&drawn, accuracy)
            });
            self.fitting = Some((accuracy, job));
        }
        fresh
    }

    /// Whether the refit at `accuracy` has landed.
    fn fitted_at(&self, accuracy: f64) -> bool {
        self.fitted.as_ref().is_some_and(|(a, _)| *a == accuracy)
    }

    /// Make the refit at `accuracy` land, waiting on the worker if it has it in hand
    /// and working it out here otherwise: the button commits what the field shows.
    fn settle(&mut self, accuracy: f64) {
        if self.fitted_at(accuracy) {
            return;
        }
        let path = match self.fitting.take() {
            Some((a, job)) if a == accuracy => job.wait(),
            _ => None,
        };
        let path = path.unwrap_or_else(|| build::simplify_outline_within(&self.drawn, accuracy));
        self.fitted = Some((accuracy, path));
    }
}

/// How many anchors `path` draws.
fn anchors_in(path: &BezPath) -> usize {
    tools::pen_anchors(path, &[])
        .iter()
        .map(|s| s.anchors.len())
        .sum()
}

impl OndinApp {
    /// Open the Clean up card on `id`, or close it. The Offset card, which hangs in
    /// the same place, closes, and so do the Clean up and Fill with content boxes,
    /// which close this card when they open.
    pub(crate) fn toggle_cleanup(&mut self, id: NodeId) {
        if self.cleanup.is_some() {
            return self.close_cleanup();
        }
        self.close_offset_card();
        let Some(card) = self.read_cleanup(id) else {
            return;
        };
        self.clean_up = None;
        self.fill_content = None;
        self.cleanup = Some(CleanupCard {
            focus_apply: true,
            ..card
        });
    }

    /// The card fresh on `id` as it is drawn now (a stretched path's drawn points).
    fn read_cleanup(&self, id: NodeId) -> Option<CleanupCard> {
        let kind = self
            .session
            .resolved
            .used_kind(&self.session.doc, id)?
            .clone();
        let NodeKind::Path {
            path,
            corner_radii,
            corner_smoothing,
            ..
        } = &kind
        else {
            return None;
        };
        let drawn =
            ondin_core::geometry::round_corners_smoothed(path, corner_radii, *corner_smoothing);
        let redundant = self
            .edited_subpaths(id)
            .map_or(0, |(_, subs)| tools::redundant_points(&subs).1.len());
        Some(CleanupCard {
            id,
            basis: kind,
            drawn: Arc::new(drawn),
            fitted: None,
            fitting: None,
            unanswered: None,
            redundant,
            shown: None,
            dropping: Vec::new(),
            focus_apply: false,
        })
    }

    /// Close the card, taking its preview off the canvas.
    pub(crate) fn close_cleanup(&mut self) {
        if let Some(card) = self.cleanup.take()
            && card.shown.is_some()
        {
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the gesture preview is the card's row rather than a gesture.
    pub(crate) fn cleanup_preview_standing(&self) -> bool {
        self.drag.is_none() && self.cleanup.as_ref().is_some_and(|c| c.shown.is_some())
    }

    /// What `row` would do to the card's path now, and the points it takes away in
    /// world space; `None` when it would change nothing.
    fn cleanup_tx(&self, card: &CleanupCard, row: Row) -> Option<(Transaction, Vec<Point>)> {
        let id = card.id;
        let n = self.cleanup_numbers;
        let (world, subpaths) = self.edited_subpaths(id)?;
        let stored = self.drawn_path(id);
        match row {
            Row::Simplify => {
                let (_, fitted) = card.fitted.as_ref()?;
                let NodeKind::Path { path, .. } = &card.basis else {
                    return None;
                };
                if fitted.segments().count() >= path.segments().count() {
                    return None;
                }
                let kept: Vec<Point> = tools::pen_anchors(fitted, &[])
                    .iter()
                    .flat_map(|s| s.anchors.iter().map(|a| world * a.at))
                    .collect();
                let dropping = subpaths
                    .iter()
                    .flat_map(|s| s.anchors.iter().map(|a| a.at))
                    .filter(|p| !kept.iter().any(|k| k.distance(*p) < 1e-6))
                    .collect();
                let tx = Transaction(vec![Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Path {
                        path: fitted.clone(),
                        corner_radii: Vec::new(),
                        mirror: None,
                    },
                }]);
                Some((tx, dropping))
            }
            Row::Remove => {
                let (list, gone) = tools::redundant_points(&subpaths);
                (!gone.is_empty()).then(|| (tools::rewrite_path(id, world, &list, stored), gone))
            }
            Row::Add => {
                let list = tools::split_segments(&subpaths, n.pieces.round() as usize);
                let tx = tools::rewrite_path(id, world, &list, stored);
                (!tx.0.is_empty()).then_some((tx, Vec::new()))
            }
            Row::Smooth => {
                let points = self.with_mirrored_points(id, &self.subject_points(id));
                if points.is_empty() {
                    return None;
                }
                let mut list = subpaths;
                tools::smooth_points(&mut list, &points, n.amount / 100.0);
                let tx = self.rewrite_edit(id, world, list);
                (!tx.0.is_empty()).then_some((tx, Vec::new()))
            }
        }
    }

    /// Commit `row`, one undo step named for it, and say what it did.
    fn commit_cleanup(&mut self, card: &mut CleanupCard, row: Row) {
        if row == Row::Simplify {
            card.settle(self.cleanup_numbers.accuracy);
        }
        if card.shown.take().is_some() {
            self.session.clear_gesture_preview();
        }
        card.dropping.clear();
        let before = card.basis.clone();
        let Some((tx, gone)) = self.cleanup_tx(card, row) else {
            return self.session.info(match row {
                Row::Simplify => "Already as simple as it gets",
                Row::Remove => "Every point shapes the path",
                Row::Add | Row::Smooth => "Nothing to change",
            });
        };
        let (label, glyph) = row.step();
        self.session.name_step(label, glyph);
        if !self.session.commit(tx) {
            return;
        }
        let said = match row {
            Row::Simplify => {
                let after = card.fitted.as_ref().map_or(0, |(_, p)| anchors_in(p));
                let NodeKind::Path { path, .. } = &before else {
                    return;
                };
                format!("Simplified: {} points → {after}", anchors_in(path))
            }
            Row::Remove => format!("Removed {}", ui::count_as(gone.len(), "point", "points")),
            Row::Add => "Added points".to_owned(),
            Row::Smooth => "Smoothed".to_owned(),
        };
        // A smooth keeps every anchor where it was; the others renumber them.
        if row != Row::Smooth {
            self.points.clear();
        }
        self.session.info(said);
    }

    /// The card beside the bar, its top on the wand's. It stays open while points
    /// are picked for *Smooth selected*; the wand, its ×, `Escape` and the end of
    /// the edit close it.
    pub(crate) fn cleanup_ui(&mut self, ctx: &egui::Context, bar: egui::Rect, button: egui::Rect) {
        let Some(mut card) = self.cleanup.take() else {
            return;
        };
        if self.edited_path() != Some(card.id) {
            self.cleanup = Some(card);
            return self.close_cleanup();
        }
        let now = self.session.resolved.used_kind(&self.session.doc, card.id);
        if now != Some(&card.basis) {
            let Some(fresh) = self.read_cleanup(card.id) else {
                self.cleanup = Some(card);
                return self.close_cleanup();
            };
            if card.shown.is_some() {
                self.session.clear_gesture_preview();
            }
            card = CleanupCard {
                focus_apply: card.focus_apply,
                ..fresh
            };
        }
        let before = self.cleanup_numbers;
        let mut n = before;
        card.pump(ctx, n.accuracy);
        let points = self.subject_points(card.id).len();
        let total = match &card.basis {
            NodeKind::Path { path, .. } => anchors_in(path),
            _ => 0,
        };
        let simplified = card
            .fitted
            .as_ref()
            .filter(|(a, _)| *a == n.accuracy)
            .map(|(_, p)| anchors_in(p));
        let note = simplify_note(ctx, &mut card, total, simplified);

        let mut hot: Option<Row> = None;
        let mut act: Option<Row> = None;
        let mut close = false;
        let mut scrubbing = false;
        let at = egui::pos2(bar.right() + crate::path_bar::GAP, button.top());
        egui::Area::new(egui::Id::new("cleanup-card"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                    let w = ui::menu_inner_w(W, ui::POPOVER_PAD);
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                    close = ui::popover_head(ui, "Clean up");
                    let bw = ["Apply", "Remove", "Add", "Smooth"]
                        .iter()
                        .map(|t| crate::settings::text_button_w(ui, t))
                        .fold(0.0, f32::max);
                    let fw = w - bw - ui::CARD_COL_GAP;
                    let field = egui::vec2(fw, ui::CONTROL_H);
                    let button = egui::vec2(bw, ui::CONTROL_H);
                    let state = |on: bool| match on {
                        true => FieldButton::Off,
                        false => FieldButton::Disabled,
                    };
                    // A row is shown while its button is hovered or focused, or its
                    // field scrubbed or typed in.
                    let mut row = |which: Row, f: Option<egui::Response>, b: egui::Response| {
                        let engaged = f.as_ref().is_some_and(|f| f.dragged() || f.has_focus());
                        scrubbing |= f.as_ref().is_some_and(|f| f.dragged());
                        if engaged || b.hovered() || b.has_focus() {
                            hot = Some(which);
                        }
                        if b.clicked() {
                            act = Some(which);
                        }
                    };

                    ui::labelled(ui, "Simplify", |ui| {
                        let (f, b) = ui
                            .horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                let (f, _) = ui::value_field_suffixed(
                                    ui,
                                    field,
                                    Prefix::Text(""),
                                    Some(Suffix {
                                        text: "px",
                                        clickable: false,
                                        tooltip: "",
                                    }),
                                    &mut n.accuracy,
                                    Scrub::fine(0.05, 2).range(build::SIMPLIFY_MIN_ACCURACY..=20.0),
                                    |d| d.custom_formatter(ui::number(2)),
                                );
                                let f = f.on_hover_text(
                                    "How far the outline may move — more drops more points",
                                );
                                let b = ui::label_button(ui, "Apply", button, FieldButton::Off);
                                (f, b)
                            })
                            .inner;
                        if std::mem::take(&mut card.focus_apply) {
                            b.request_focus();
                        }
                        caption(ui, &note);
                        row(Row::Simplify, Some(f), b);
                    });

                    ui::labelled(ui, "Remove redundant points", |ui| {
                        let b = ui
                            .horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                let (r, _) = ui.allocate_exact_size(field, egui::Sense::hover());
                                let note = match card.redundant {
                                    0 => "Nothing to remove".to_owned(),
                                    k => format!("{} can go", ui::count_as(k, "point", "points")),
                                };
                                ui.painter().text(
                                    egui::pos2(r.left(), r.center().y),
                                    egui::Align2::LEFT_CENTER,
                                    note,
                                    egui::FontId::proportional(ui::CONTROL_PT),
                                    theme::text::muted(),
                                );
                                ui::label_button(ui, "Remove", button, state(card.redundant > 0))
                            })
                            .inner;
                        let b = match card.redundant {
                            0 => b.on_hover_text("Every point shapes the path"),
                            _ => b.on_hover_text("Take out the points the shape doesn't need"),
                        };
                        // Always shown, whatever the count, so the card keeps its height.
                        caption(
                            ui,
                            "Points along a straight line, doubled points, and points that \
                             split one curve in two. The shape stays exactly as it is.",
                        );
                        row(Row::Remove, None, b);
                    });

                    ui::labelled(ui, "Add points", |ui| {
                        let (f, b) = ui
                            .horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                let f = ui::value_field(
                                    ui,
                                    field,
                                    Prefix::Text(""),
                                    &mut n.pieces,
                                    Scrub::whole(0.1).range(2.0..=MAX_PIECES),
                                    |d| d.custom_formatter(ui::number(0)),
                                )
                                .on_hover_text("How many equal pieces each segment is cut into");
                                let b = ui::label_button(ui, "Add", button, FieldButton::Off);
                                (f, b)
                            })
                            .inner;
                        row(Row::Add, Some(f), b);
                    });

                    ui::labelled(ui, "Smooth selected", |ui| {
                        let (f, b) = ui
                            .horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                                // Dimmed with its button: with no points there is
                                // nothing for the amount to act on.
                                let f = ui
                                    .add_enabled_ui(points > 0, |ui| {
                                        ui::value_field(
                                            ui,
                                            field,
                                            Prefix::Text(""),
                                            &mut n.amount,
                                            Scrub::whole(0.5).range(0.0..=100.0),
                                            |d| d.custom_formatter(|v, _| format!("{v:.0}%")),
                                        )
                                    })
                                    .inner
                                    .on_hover_text("How far the handles go toward smooth")
                                    .on_disabled_hover_text("Select points to smooth them");
                                let b = ui::label_button(ui, "Smooth", button, state(points > 0));
                                (f, b)
                            })
                            .inner;
                        let b = match points {
                            0 => b.on_hover_text("Select points to smooth them"),
                            _ => b,
                        };
                        let note = match points {
                            0 => "Select points to smooth them".to_owned(),
                            k => format!(
                                "Acts on the {}",
                                ui::count_as(k, "selected point", "selected points")
                            ),
                        };
                        caption(ui, &note);
                        row(Row::Smooth, Some(f), b);
                    });
                });
            });

        // Armed after a right-click's cancel is answered, so the frame it lands on
        // cannot re-arm it.
        if scrubbing && !self.gesture_cancelled && self.in_flight.cleanup_scrub.is_none() {
            self.in_flight.cleanup_scrub = Some(before);
        }
        self.cleanup_numbers = n;
        if close {
            self.cleanup = Some(card);
            return self.close_cleanup();
        }
        if let Some(row) = act.filter(|r| self.cleanup_row_live(&card, *r, points)) {
            self.commit_cleanup(&mut card, row);
            self.cleanup = Some(card);
            ctx.request_repaint();
            return;
        }
        let hot = hot.filter(|r| self.cleanup_row_live(&card, *r, points));
        self.show_cleanup_row(&mut card, hot);
        self.cleanup = Some(card);
    }

    /// Whether `row`'s button takes a click.
    fn cleanup_row_live(&self, card: &CleanupCard, row: Row, points: usize) -> bool {
        match row {
            Row::Remove => card.redundant > 0,
            Row::Smooth => points > 0,
            Row::Simplify | Row::Add => true,
        }
    }

    /// Put `row`'s result on the canvas, built again only when what it shows
    /// changed, or take the card's off when `row` is `None`. Hands off while a
    /// canvas drag owns the preview.
    fn show_cleanup_row(&mut self, card: &mut CleanupCard, row: Option<Row>) {
        if !self.drag.is_none() {
            card.shown = None;
            card.dropping.clear();
            return;
        }
        let Some(row) = row else {
            if card.shown.take().is_some() {
                self.session.clear_gesture_preview();
            }
            card.dropping.clear();
            return;
        };
        let key: ShownKey = (
            self.cleanup_numbers,
            self.subject_points(card.id),
            card.fitted.as_ref().map(|(a, _)| *a),
        );
        if card.shown.as_ref() == Some(&(row, key.clone())) && self.session.has_gesture_preview() {
            return;
        }
        match self.cleanup_tx(card, row) {
            Some((tx, gone)) => {
                self.session.set_preview(&tx);
                card.dropping = gone;
            }
            None => {
                self.session.clear_gesture_preview();
                card.dropping.clear();
            }
        }
        card.shown = Some((row, key));
    }

    /// The points the card's shown row would take away, for the canvas to mark.
    pub(crate) fn cleanup_dropping(&self) -> &[Point] {
        self.cleanup.as_ref().map_or(&[], |c| &c.dropping)
    }
}

/// Simplify's caption: the count at the field's accuracy once its refit has landed.
/// Before that it keeps the last count that landed (or the path's own, before any
/// has), and says *Fitting…* only after [`FIT_PATIENCE`].
fn simplify_note(
    ctx: &egui::Context,
    card: &mut CleanupCard,
    total: usize,
    simplified: Option<usize>,
) -> String {
    if let Some(after) = simplified {
        card.unanswered = None;
        return format!("{total} points → {after}");
    }
    let now = ctx.input(|i| i.time);
    let left = FIT_PATIENCE - (now - *card.unanswered.get_or_insert(now));
    if left <= 0.0 {
        return "Fitting…".to_owned();
    }
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
    match &card.fitted {
        Some((_, path)) => format!("{total} points → {}", anchors_in(path)),
        None => format!("{total} points"),
    }
}

/// A row's note under its controls.
fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(ui::SMALL_PT)
            .color(theme::text::muted()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_bar::tests::{app_with_a_ring, stored};
    use crate::repeat::tests::{pass, scrub_then_right_click};

    /// How many anchors the ring stores.
    fn anchors(app: &OndinApp, id: NodeId) -> usize {
        let (path, radii) = stored(app, id);
        tools::pen_anchors(&path, &radii)
            .iter()
            .map(|s| s.anchors.len())
            .sum()
    }

    /// The last undo step's name.
    fn last_step(app: &OndinApp) -> String {
        let step = app.session.history.done().last().expect("a step");
        step.note.label.clone()
    }

    /// Commit `row` on the open card.
    fn commit(app: &mut OndinApp, row: Row) {
        let mut card = app.cleanup.take().expect("the card is open");
        app.commit_cleanup(&mut card, row);
        app.cleanup = Some(card);
    }

    /// The wand opens the card with Simplify's button focused, so `Enter` refits
    /// the ring at once: one *Simplify* step, the old point selection dropped.
    ///
    /// flip: `toggle_cleanup` opening without `focus_apply` → fails at "simplified".
    #[test]
    fn the_wand_then_enter_simplifies() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        app.points.set_one(id, (0, 3));
        pass(&ctx, &mut app, Vec::new());
        app.toggle_cleanup(id);
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new());
        }
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        pass(&ctx, &mut app, vec![enter(true)]);
        pass(&ctx, &mut app, vec![enter(false)]);
        assert!(
            anchors(&app, id) < 64 / 4,
            "simplified: {}",
            anchors(&app, id)
        );
        assert_eq!(last_step(&app), "Simplify");
        assert!(app.points.is_empty(), "the old anchors are gone");
        assert!(app.cleanup.is_some(), "the card stays open");
    }

    /// A right-click mid-scrub of the accuracy puts it back and leaves the path as
    /// it was, with the pointer on the card or out over the canvas.
    ///
    /// flip: `cancel_gesture` not restoring `cleanup_scrub` → fails at "the
    /// accuracy is back".
    #[test]
    fn a_right_click_winds_an_accuracy_scrub_back() {
        for outside in [false, true] {
            let ctx = egui::Context::default();
            let (mut app, id) = app_with_a_ring(&ctx);
            pass(&ctx, &mut app, Vec::new());
            app.toggle_cleanup(id);
            let before = app.cleanup_numbers;
            let path = stored(&app, id);
            scrub_then_right_click(
                &ctx,
                &mut app,
                egui::Id::new("cleanup-card"),
                "Simplify",
                |at| match outside {
                    true => egui::pos2(700.0, 500.0),
                    false => at,
                },
                |app| {
                    assert_ne!(app.cleanup_numbers, before, "the drag scrubbed it");
                    assert!(app.in_flight.cleanup_scrub.is_some(), "the scrub is armed");
                },
            );
            assert_eq!(
                app.cleanup_numbers, before,
                "the accuracy is back ({outside})"
            );
            assert_eq!(stored(&app, id), path, "the path is as it was");
            assert!(!app.session.has_gesture_preview(), "no preview left");
            assert!(app.cleanup.is_some(), "the right-click closed nothing");
        }
    }

    /// *Add points* halves every segment of the ring but the rounded first corner's
    /// two legs in one *Add points* step, and *Remove redundant points* takes exactly
    /// those back out in one *Remove points* step; the rounded corner stays throughout.
    #[test]
    fn adding_then_removing_points_comes_back_to_the_ring() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        let path = stored(&app, id);
        app.toggle_cleanup(id);
        commit(&mut app, Row::Add);
        assert_eq!(anchors(&app, id), 126);
        assert_eq!(last_step(&app), "Add points");
        // The card reads the path again on its next frame.
        pass(&ctx, &mut app, Vec::new());
        assert_eq!(app.cleanup.as_ref().map(|c| c.redundant), Some(62));
        commit(&mut app, Row::Remove);
        assert_eq!(last_step(&app), "Remove points");
        assert_eq!(anchors(&app, id), 64);
        let (now, radii) = stored(&app, id);
        assert_eq!(
            radii.first(),
            Some(&2.0),
            "the rounded corner kept its radius"
        );
        assert!(radii[1..].iter().all(|r| *r == 0.0), "{radii:?}");
        assert_eq!(now.segments().count(), path.0.segments().count());
    }

    /// A row is shown on the canvas only while it is hot, with the points it would
    /// take away marked, and taken off again after.
    #[test]
    fn a_row_is_shown_only_while_it_is_hot() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        app.toggle_cleanup(id);
        let mut card = app.cleanup.take().expect("open");
        // Some points to remove: the card's own *Add points*, committed.
        app.commit_cleanup(&mut card, Row::Add);
        let mut card = app.read_cleanup(id).expect("read again");
        app.show_cleanup_row(&mut card, Some(Row::Remove));
        assert!(app.session.has_gesture_preview(), "shown");
        assert_eq!(
            card.dropping.len(),
            62,
            "the points that would go are marked"
        );
        app.show_cleanup_row(&mut card, None);
        assert!(!app.session.has_gesture_preview(), "taken off");
        assert!(card.dropping.is_empty());
    }

    /// A new accuracy keeps the last count under Simplify while its refit is in
    /// flight, rather than flashing *Fitting…* for a frame.
    ///
    /// flip: `FIT_PATIENCE` at 0 → fails at "no flash".
    #[test]
    fn a_new_accuracy_does_not_flash_fitting() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        pass(&ctx, &mut app, Vec::new());
        app.toggle_cleanup(id);
        let accuracy = app.cleanup_numbers.accuracy;
        let mut card = app.cleanup.take().expect("open");
        card.settle(accuracy);
        app.cleanup = Some(card);
        let painted = |out: &egui::FullOutput, f: &dyn Fn(&str) -> bool| {
            out.shapes
                .iter()
                .any(|c| matches!(&c.shape, egui::Shape::Text(t) if f(t.galley.text())))
        };
        // The card's first frame is egui's sizing pass, which paints nothing.
        pass(&ctx, &mut app, Vec::new());
        let landed = pass(&ctx, &mut app, Vec::new());
        assert!(
            painted(&landed, &|s| s.starts_with("64 points → ")),
            "fixture: the first refit has landed"
        );
        app.cleanup_numbers.accuracy = accuracy + 3.0;
        let out = pass(&ctx, &mut app, Vec::new());
        assert!(
            app.cleanup.as_ref().is_some_and(|c| c.fitting.is_some()),
            "fixture: the new refit is in flight"
        );
        assert!(!painted(&out, &|s| s == "Fitting…"), "no flash");
        assert!(painted(&out, &|s| s.starts_with("64 points → ")));
    }

    /// Drag across the field under the card's `label` eyebrow.
    fn drag_field(ctx: &egui::Context, app: &mut OndinApp, label: &str) {
        let mut out = pass(ctx, app, Vec::new());
        for _ in 0..10 {
            out = pass(ctx, app, Vec::new());
        }
        let eyebrow = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == label.to_uppercase() => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the {label} eyebrow"));
        let field = egui::pos2(
            eyebrow.left() + 40.0,
            eyebrow.bottom() + ui::SECTION_LABEL_GAP + ui::CONTROL_H / 2.0,
        );
        let button = |pressed, pos| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        pass(ctx, app, vec![egui::Event::PointerMoved(field)]);
        pass(ctx, app, vec![button(true, field)]);
        for dx in [10.0, 30.0, 60.0] {
            pass(
                ctx,
                app,
                vec![egui::Event::PointerMoved(field + egui::vec2(dx, 0.0))],
            );
        }
        pass(ctx, app, vec![button(false, field + egui::vec2(60.0, 0.0))]);
        pass(ctx, app, Vec::new());
    }

    /// With no points selected, Smooth's amount is dimmed with its button and a
    /// drag across it moves nothing; with one selected, the same drag scrubs it.
    ///
    /// flip: the amount field drawn enabled always → fails at "dimmed".
    #[test]
    fn smooth_s_amount_is_dimmed_with_no_points() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_ring(&ctx);
        pass(&ctx, &mut app, Vec::new());
        app.toggle_cleanup(id);
        let before = app.cleanup_numbers.amount;
        drag_field(&ctx, &mut app, "Smooth selected");
        assert_eq!(app.cleanup_numbers.amount, before, "dimmed");

        app.points.set_one(id, (0, 3));
        drag_field(&ctx, &mut app, "Smooth selected");
        assert_ne!(
            app.cleanup_numbers.amount, before,
            "fixture: the drag reaches the field"
        );
    }
}
