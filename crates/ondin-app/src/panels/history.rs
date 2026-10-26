//! The History section at the foot of the layers pane: the undo stack as a list,
//! one row per step, oldest at the top. A click undoes or redoes to just after
//! that row in one go (`EditorSession::jump_to`); the rows past the current one
//! are the redo stack, dimmed. Hovering a row outlines the layers its step touched
//! on this page. It lists the history that exists and adds no versions or
//! branches; it lasts as long as the document is open.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{FOLD_HEAD_H, baseline_in, fold_head};
use eframe::egui;
use ondin_core::{NodeId, Origin};

/// A row's height — the layer tree's.
const ROW_H: f32 = 26.0;
/// The open section's least height: the divider, the header's top inset, the fold
/// head, and the gap under it.
const HEAD_H: f32 = 1.0 + 10.0 + FOLD_HEAD_H + 7.0;
/// The folded header's inset above and below the fold head, equal so it sits
/// centred between the divider and the pane's foot.
const FOLDED_PAD: i8 = 12;
/// The whole folded section: divider, insets and fold head.
const FOLDED_H: f32 = 1.0 + 2.0 * FOLDED_PAD as f32 + FOLD_HEAD_H;
/// The section's height until its divider is dragged, header included.
const DEFAULT_H: f32 = 240.0;
/// The divider's grab band, centred on its hairline.
const DIVIDER_GRAB: f32 = 6.0;
/// The least room the pane keeps above the section when nothing has been
/// measured yet.
const ABOVE_FALLBACK: f32 = 120.0;
/// How far a redo row's ink is dimmed.
const AHEAD_DIM: f32 = 0.38;

/// The History section's own state.
#[derive(Default)]
pub(crate) struct HistoryState {
    /// The layers the hovered row's step touched, on the current page and still
    /// there; the canvas outlines them this frame. Cleared at each frame's start.
    pub(crate) hover: Vec<NodeId>,
    /// Where the Layers header ended last frame: the divider may not cover it, or
    /// the Pages section above it. `0.0` until measured.
    pub(crate) head_bottom: f32,
    /// The current step and the length of the list when they were last scrolled
    /// into view.
    shown: Option<(usize, usize)>,
    /// Each drawn row's depth and rect, for the tests to aim at.
    #[cfg(test)]
    pub(crate) rows: Vec<(usize, egui::Rect)>,
    /// The Layers tree's viewport last frame, which must end above this section.
    #[cfg(test)]
    pub(crate) tree_view: Option<egui::Rect>,
    /// The section's rect and its fold head's, last frame.
    #[cfg(test)]
    pub(crate) drawn: Option<(egui::Rect, egui::Rect)>,
}

/// One row, read off the history before any of it is drawn.
struct Row {
    /// Where the row stands the document: after this many done steps.
    depth: usize,
    /// The step's serial, which a click aims at; `None` for the first row.
    serial: Option<u64>,
    glyph: std::borrow::Cow<'static, str>,
    label: String,
    /// The step's subject and how many more it touched.
    subject: Option<(String, usize)>,
    /// The page the step happened on, when that is not the page in view.
    page: Option<String>,
    saved: bool,
    ahead: bool,
    mcp: Option<(String, String)>,
    subjects: Vec<NodeId>,
}

impl OndinApp {
    /// The section, docked at the bottom of the layers pane. Called before the pane's
    /// other sections; answers the room above it, which they take.
    ///
    /// Laid out by hand rather than as an `egui::Panel`: a panel's item spacing pushed
    /// its content past the pane's foot and left the tree above reaching into it.
    pub(crate) fn history_section(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let folded = self.prefs.history_folded;
        let pane = ui.max_rect();
        let floor = match self.history_pane.head_bottom {
            y if y > pane.top() => y,
            _ => pane.top() + ABOVE_FALLBACK,
        };
        let max = (pane.bottom() - floor).max(HEAD_H);
        let height = match folded {
            true => FOLDED_H,
            false => self
                .prefs
                .history_height
                .unwrap_or(DEFAULT_H)
                .clamp(HEAD_H, max),
        };
        let rect =
            egui::Rect::from_min_max(egui::pos2(pane.left(), pane.bottom() - height), pane.max);
        let mut section = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        section.set_clip_rect(rect.intersect(ui.clip_rect()));
        // The insets are the section's own; spacing between its parts would push the
        // head off its centre and the list past the pane's foot.
        section.spacing_mut().item_spacing.y = 0.0;
        self.history_divider(&mut section, folded, max);
        let head = self.history_head(&mut section, folded);
        if !folded {
            self.history_list(&mut section);
        }
        #[cfg(test)]
        {
            self.history_pane.drawn = Some((rect, head));
        }
        let _ = head;
        egui::Rect::from_min_max(pane.min, egui::pos2(pane.max.x, rect.top()))
    }

    /// The hairline above the section, and — while it is open — a band that drags
    /// its height, kept as a preference. Not capped at the rows' height: History may
    /// be taller than its list, and Layers as short as its header.
    fn history_divider(&mut self, ui: &mut egui::Ui, folded: bool, max: f32) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, color::DIVIDER);
        if folded {
            return;
        }
        let band = rect.expand2(egui::vec2(0.0, DIVIDER_GRAB / 2.0));
        let resp = ui.interact(band, egui::Id::new("history-divider"), egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if resp.dragged() {
            let now = self.prefs.history_height.unwrap_or(DEFAULT_H).min(max);
            // Up grows the section: the divider is its top edge.
            self.prefs.history_height = Some((now - resp.drag_delta().y).clamp(HEAD_H, max));
        }
        if resp.drag_stopped() {
            self.prefs.save();
        }
    }

    /// The fold head. Folded, it names the current step and how many are ahead.
    /// Answers the fold head's rect.
    fn history_head(&mut self, ui: &mut egui::Ui, folded: bool) -> egui::Rect {
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: crate::panels::layers::PAD_X,
                right: crate::panels::layers::PAD_X,
                top: if folded { FOLDED_PAD } else { 10 },
                bottom: if folded { FOLDED_PAD } else { 7 },
            })
            .show(ui, |ui| {
                // The row is the fold head's height, so the insets alone place it.
                ui.spacing_mut().interact_size.y = FOLD_HEAD_H;
                ui.horizontal(|ui| {
                    let (head, baseline) = fold_head(ui, "History", !folded);
                    if head.clicked() {
                        self.prefs.history_folded = !folded;
                        self.prefs.save();
                    }
                    if !folded {
                        return head.rect;
                    }
                    let ahead = self.session.history.redo_depth();
                    let tail = (ahead > 0).then(|| format!("{ahead} ahead"));
                    let tail = tail.map(|t| {
                        ui.painter().layout_no_wrap(
                            t,
                            egui::FontId::proportional(10.5),
                            theme::text::DIM,
                        )
                    });
                    ui.add_space(4.0);
                    let room =
                        ui.available_width() - tail.as_ref().map_or(0.0, |t| t.size().x + 8.0);
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        self.current_step_text(),
                        egui::FontId::proportional(12.0),
                        theme::text::MUTED,
                    );
                    job.wrap = egui::text::TextWrapping::truncate_at_width(room.max(0.0));
                    let name = ui.painter().layout_job(job);
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(name.size().x, FOLD_HEAD_H),
                        egui::Sense::hover(),
                    );
                    let top = baseline - baseline_in(&name);
                    ui.painter()
                        .galley(egui::pos2(rect.left(), top), name, theme::text::MUTED);
                    if let Some(tail) = tail {
                        let right = ui.max_rect().right();
                        let top = baseline - baseline_in(&tail);
                        ui.painter().galley(
                            egui::pos2(right - tail.size().x, top),
                            tail,
                            theme::text::DIM,
                        );
                    }
                    head.rect
                })
                .inner
            })
            .inner
    }

    /// The step the document stands just after, as its row reads.
    fn current_step_text(&self) -> String {
        match self.session.history.done().last() {
            Some(step) => step_text(
                &step.note.label,
                subject_of(&step.note.subject, step.note.subjects.len()).as_ref(),
            ),
            None => self.first_row_label().to_string(),
        }
    }

    /// The first row: the document as opened, or — with a history kept from an
    /// earlier session — as it stood before the oldest step kept.
    fn first_row_label(&self) -> &'static str {
        match (
            self.session.resumed_history(),
            self.session.opened_from_file(),
        ) {
            (true, _) => "Earlier",
            (false, true) => "Opened",
            (false, false) => "New document",
        }
    }

    /// Every row, oldest first: the first row, the done steps, then the undone ones.
    fn history_rows(&self) -> Vec<Row> {
        let history = &self.session.history;
        let saved = self.session.saved_step();
        let here = history.undo_depth();
        let doc = &self.session.doc;
        let page = self.session.page;
        let mut rows = vec![Row {
            depth: 0,
            serial: None,
            glyph: match self.session.resumed_history() {
                true => icon::CLOCK_COUNTER_CLOCKWISE.into(),
                false => icon::FOLDER_OPEN.into(),
            },
            label: self.first_row_label().into(),
            subject: None,
            page: None,
            saved: saved == Some(None),
            ahead: false,
            mcp: None,
            subjects: Vec::new(),
        }];
        for (i, step) in history.done().chain(history.undone()).enumerate() {
            let note = step.note;
            let first = note.subjects.first().copied();
            let elsewhere = note
                .page
                .filter(|p| *p != page && Some(*p) != first && doc.is_page(*p));
            rows.push(Row {
                depth: i + 1,
                serial: Some(step.serial),
                glyph: match note.glyph.is_empty() {
                    true => icon::PENCIL_SIMPLE.into(),
                    false => note.glyph.clone(),
                },
                label: note.label.clone(),
                subject: subject_of(&note.subject, note.subjects.len()),
                page: elsewhere.map(|p| self.page_name(p)),
                saved: saved == Some(Some(step.serial)),
                ahead: i >= here,
                mcp: match &note.origin {
                    Origin::Mcp { client, tool } => Some((client.clone(), tool.clone())),
                    Origin::User => None,
                },
                subjects: note.subjects.clone(),
            });
        }
        rows
    }

    /// The rows, in a list that keeps the current one in view.
    fn history_list(&mut self, ui: &mut egui::Ui) {
        let rows = self.history_rows();
        let here = self.session.history.undo_depth();
        let key = (here, rows.len());
        let scroll = self.history_pane.shown != Some(key);
        self.history_pane.shown = Some(key);
        #[cfg(test)]
        self.history_pane.rows.clear();
        let list = egui::ScrollArea::vertical()
            .id_salt("history")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .show(ui, |ui| {
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: crate::panels::layers::TREE_PAD_X,
                        right: crate::panels::layers::TREE_PAD_X,
                        top: 0,
                        bottom: 8,
                    })
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for row in &rows {
                            let rect = self.history_row(ui, row, row.depth == here);
                            #[cfg(test)]
                            self.history_pane.rows.push((row.depth, rect));
                            if scroll && row.depth == here {
                                ui.scroll_to_rect(rect, None);
                            }
                        }
                    });
            });
        crate::ui::scroll_fade_top(ui, list.inner_rect, list.state.offset.y, color::PANEL);
    }

    /// One row; answers its rect.
    fn history_row(&mut self, ui: &mut egui::Ui, row: &Row, current: bool) -> egui::Rect {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::click(),
        );
        let resp = match &row.mcp {
            Some((client, tool)) => resp.on_hover_ui(|ui| {
                ui.label(format!("Made over MCP by {client}"));
                ui.label(
                    egui::RichText::new(mcp_detail(tool, row.subjects.len(), row.page.as_deref()))
                        .color(theme::text::DIM),
                );
            }),
            None => resp,
        };
        let hovered = resp.hovered();
        if hovered {
            let doc = &self.session.doc;
            let page = self.session.page;
            self.history_pane.hover = row
                .subjects
                .iter()
                .copied()
                .filter(|id| *id != page && doc.page_of(*id) == Some(page))
                .collect();
        }
        let dim = |c: egui::Color32| match row.ahead {
            true => c.gamma_multiply(AHEAD_DIM),
            false => c,
        };
        let p = ui.painter();
        let r5 = egui::CornerRadius::same(5);
        if current {
            p.rect_filled(rect, r5, color::ACCENT.gamma_multiply(0.22));
        } else if hovered {
            p.rect_filled(rect, r5, theme::color::text_a(12));
        }
        let lit = current || hovered;
        let cy = rect.center().y;
        let mut x = rect.left() + 8.0;
        p.text(
            egui::pos2(x + 7.0, cy),
            egui::Align2::CENTER_CENTER,
            &row.glyph,
            theme::icon_font(14.0),
            dim(if current {
                color::ACCENT
            } else {
                theme::text::DIM
            }),
        );
        x += 14.0 + 7.0;

        // The right side first, so the subject can be cut to what is left.
        let mut right = rect.right() - 8.0;
        if row.mcp.is_some() {
            let g = p.layout_no_wrap(
                icon::SPARKLE.into(),
                theme::icon_font(12.0),
                dim(color::ACCENT),
            );
            right -= g.size().x;
            p.galley(egui::pos2(right, cy - g.size().y / 2.0), g, color::ACCENT);
            right -= 6.0;
        }
        if row.saved {
            let tag = p.layout_no_wrap(
                "Saved".to_string(),
                egui::FontId::proportional(10.0),
                dim(theme::text::MUTED),
            );
            let size = tag.size() + egui::vec2(10.0, 3.0);
            let at = egui::Rect::from_min_size(egui::pos2(right - size.x, cy - size.y / 2.0), size);
            p.rect_stroke(
                at,
                egui::CornerRadius::same(3),
                egui::Stroke::new(1.0, dim(theme::color::text_a(40))),
                egui::StrokeKind::Inside,
            );
            p.galley(at.min + egui::vec2(5.0, 1.5), tag, theme::text::MUTED);
            right = at.left() - 6.0;
        }

        // `Label · Subject · Page`: the subject gives way first, then the page.
        let font = egui::FontId::proportional(12.5);
        let ink = dim(if lit { color::TEXT } else { theme::text::MUTED });
        let sub = dim(if lit {
            theme::text::MUTED
        } else {
            theme::text::DIM
        });
        let cut = |text: String, ink: egui::Color32, width: f32| {
            let mut job = egui::text::LayoutJob::simple_singleline(text, font.clone(), ink);
            job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
            p.layout_job(job)
        };
        let room = (right - x).max(0.0);
        let label = cut(row.label.clone(), ink, room);
        let subject = row.subject.as_ref().map(|(name, more)| match more {
            0 => format!(" · {name}"),
            n => format!(" · {name} +{n}"),
        });
        let page = row.page.as_ref().map(|page| format!(" · {page}"));
        let natural = |text: &Option<String>| {
            text.as_ref()
                .map_or(0.0, |t| cut(t.clone(), sub, f32::INFINITY).size().x)
        };
        let least = cut(" · …".into(), sub, f32::INFINITY).size().x;
        let (subject_w, page_w) = share(
            room - label.size().x,
            natural(&subject),
            natural(&page),
            least,
        );
        let parts = [
            Some(label),
            subject.map(|t| cut(t, sub, subject_w)),
            page.map(|t| cut(t, sub, page_w)),
        ];
        for part in parts.into_iter().flatten() {
            let w = part.size().x;
            // Each part's ink is in its galley.
            p.galley(egui::pos2(x, cy - part.size().y / 2.0), part, ink);
            x += w;
        }

        if resp.clicked() && !current {
            self.jump_history(row.serial);
        }
        rect
    }
}

/// How a row's `rest` of width after its label splits between the subject and the
/// page, from what each would take whole: the subject gives way first, down to
/// `least` (a bare ` · …`), then the page.
fn share(rest: f32, subject: f32, page: f32, least: f32) -> (f32, f32) {
    let rest = rest.max(0.0);
    if subject + page <= rest {
        return (subject, page);
    }
    let subject = (rest - page).max(least.min(subject)).min(subject);
    (subject, (rest - subject).max(0.0))
}

/// A row's text after its label: the subject's name and how many more.
fn subject_of(name: &Option<String>, count: usize) -> Option<(String, usize)> {
    name.as_ref().map(|n| (n.clone(), count.saturating_sub(1)))
}

/// A step as one line: `Move · Card +1`.
fn step_text(label: &str, subject: Option<&(String, usize)>) -> String {
    match subject {
        None => label.to_string(),
        Some((name, 0)) => format!("{label} · {name}"),
        Some((name, more)) => format!("{label} · {name} +{more}"),
    }
}

/// An MCP row's tooltip, under who made it: the tool and what it touched.
fn mcp_detail(tool: &str, layers: usize, page: Option<&str>) -> String {
    let what = match layers {
        0 => String::new(),
        1 => " · 1 layer".to_string(),
        n => format!(" · {n} layers"),
    };
    match page {
        Some(page) if layers > 0 => format!("{tool}{what} in {page}"),
        _ => format!("{tool}{what}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{NodeKind, Operation, Transaction};
    use std::path::PathBuf;

    /// A headless app with a rect on its first artboard, named `Card`.
    fn app_with_card(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let s = &mut app.session;
        let parent = s.doc.get(s.page).unwrap().children()[0];
        let card = s.ids.mint();
        s.name_step("Add rectangle", icon::SQUARE);
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id: card,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Card".into()),
        }])));
        (app, card)
    }

    fn opacity(app: &mut OndinApp, id: NodeId, opacity: f32) {
        app.session.name_step("Opacity", icon::DROP_HALF);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetOpacity { id, opacity }]))
        );
    }

    fn whole_frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let time = ctx.input(|i| i.time) + 0.1;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| eframe::App::ui(app, ui, &mut frame),
        );
    }

    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        whole_frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        whole_frame(ctx, app, vec![button(true)]);
        whole_frame(ctx, app, vec![button(false)]);
    }

    fn shown(ctx: &egui::Context) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let (mut app, card) = app_with_card(ctx);
        app.view = crate::app::View::Editor;
        app.show_history = true;
        (app, card)
    }

    /// The rows read oldest first under the first row, the redo rows are ahead, and
    /// *Saved* sits on the step whose state is on disk.
    #[test]
    fn the_rows_read_the_history_oldest_first() {
        let ctx = egui::Context::default();
        let (mut app, card) = app_with_card(&ctx);
        opacity(&mut app, card, 0.5);
        app.session.mark_saved(PathBuf::from("x.ondin"));
        opacity(&mut app, card, 0.25);
        assert!(app.session.undo());

        let rows = app.history_rows();
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["New document", "Add rectangle", "Opacity", "Opacity"]
        );
        let ahead: Vec<_> = rows.iter().map(|r| r.ahead).collect();
        assert_eq!(ahead, [false, false, false, true]);
        let saved: Vec<_> = rows.iter().map(|r| r.saved).collect();
        assert_eq!(saved, [false, false, true, false]);
        assert_eq!(rows[1].subject, Some(("Card".to_string(), 0)));
        assert_eq!(app.current_step_text(), "Opacity · Card");
    }

    /// Folded, the section ends at the pane's foot with its head centred between the
    /// divider and the foot, and the tree above stops at the divider.
    /// flip: the section back on `item_spacing` (no zeroing in `history_section`) →
    /// fails at "head centred".
    #[test]
    fn the_folded_section_sits_centred_at_the_foot() {
        let ctx = egui::Context::default();
        let (mut app, _) = shown(&ctx);
        app.prefs.history_folded = true;
        for _ in 0..4 {
            whole_frame(&ctx, &mut app, vec![]);
        }
        let (section, head) = app.history_pane.drawn.expect("the section was drawn");
        assert_eq!(section.bottom(), 820.0, "on the pane's foot");
        let above = head.top() - (section.top() + 1.0);
        let below = section.bottom() - head.bottom();
        assert_eq!(above, below, "head centred: {above} above, {below} below");
        let tree = app.history_pane.tree_view.expect("the tree was drawn");
        assert!(
            tree.bottom() <= section.top(),
            "tree to {} over {}",
            tree.bottom(),
            section.top()
        );
    }

    /// A row short of room cuts the layer name before the page, down to a bare
    /// ` · …`, and only then the page.
    #[test]
    fn the_subject_gives_way_before_the_page() {
        assert_eq!(share(200.0, 80.0, 50.0, 12.0), (80.0, 50.0), "all fits");
        assert_eq!(share(100.0, 80.0, 50.0, 12.0), (50.0, 50.0), "subject cut");
        assert_eq!(share(40.0, 80.0, 50.0, 12.0), (12.0, 28.0), "then the page");
        assert_eq!(share(40.0, 0.0, 50.0, 12.0), (0.0, 40.0), "no subject");
        assert_eq!(share(-5.0, 80.0, 50.0, 12.0), (12.0, 0.0), "no room at all");
    }

    /// A jump aims at a step, not a position: a commit between reading the row and
    /// jumping (a text session finished first) moves it, or drops it with the redo
    /// rows, and the jump follows or does nothing.
    /// flip: `jump_history` falling back to the first row for a serial it cannot find
    /// → fails at "a dropped row jumps nowhere".
    #[test]
    fn a_jump_aims_at_its_step_not_its_position() {
        let ctx = egui::Context::default();
        let (mut app, card) = app_with_card(&ctx);
        opacity(&mut app, card, 0.5);
        let half = app.session.history.current();
        opacity(&mut app, card, 0.25);
        assert!(app.session.undo());
        let dropped = app.session.history.undone().next().map(|v| v.serial);
        opacity(&mut app, card, 0.75);

        let depth = app.session.history.undo_depth();
        app.jump_history(dropped);
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "a dropped row jumps nowhere"
        );
        app.jump_history(half);
        assert_eq!(app.session.history.current(), half);
        assert_eq!(app.session.doc.get(card).unwrap().opacity(), 0.5);
    }

    /// A click on a row stands the document just after it, in one go; a click on a
    /// dimmed row redoes up to it.
    #[test]
    fn a_click_on_a_row_jumps_to_it() {
        let ctx = egui::Context::default();
        let (mut app, card) = shown(&ctx);
        for o in [0.8, 0.6, 0.4] {
            opacity(&mut app, card, o);
        }
        for _ in 0..3 {
            whole_frame(&ctx, &mut app, vec![]);
        }
        let row = |app: &OndinApp, depth| {
            app.history_pane
                .rows
                .iter()
                .find(|(d, _)| *d == depth)
                .map(|(_, r)| r.center())
                .expect("the row was drawn")
        };
        let at = row(&app, 1);
        click(&ctx, &mut app, at);
        assert_eq!(app.session.history.undo_depth(), 1);
        assert_eq!(app.session.doc.get(card).unwrap().opacity(), 1.0);
        assert_eq!(app.session.status().text, "Undid 3 steps · Ctrl+Y to redo");

        for _ in 0..40 {
            whole_frame(&ctx, &mut app, vec![]);
        }
        let at = row(&app, 3);
        click(&ctx, &mut app, at);
        assert_eq!(app.session.doc.get(card).unwrap().opacity(), 0.6);
        assert_eq!(app.session.history.redo_depth(), 1);
    }

    /// Hovering a row outlines its step's layers on this page, and nothing for a
    /// step whose layers are elsewhere.
    /// flip: `history_row`'s hover filter without its page test → fails at "the page
    /// itself draws nothing".
    #[test]
    fn hovering_a_row_names_its_layers_on_this_page() {
        let ctx = egui::Context::default();
        let (mut app, card) = shown(&ctx);
        opacity(&mut app, card, 0.5);
        let other = app.session.ids.mint();
        app.session.name_step("Add page", icon::FILE);
        assert!(app.session.commit(Transaction(vec![Operation::AddPage {
            id: other,
            index: 1,
            name: "Archive".into(),
            ground: ondin_core::DEFAULT_CANVAS_BACKGROUND,
            draft: false,
            mode: None,
        }])));
        for _ in 0..3 {
            whole_frame(&ctx, &mut app, vec![]);
        }
        let at = |app: &OndinApp, depth| {
            app.history_pane
                .rows
                .iter()
                .find(|(d, _)| *d == depth)
                .unwrap()
                .1
                .center()
        };
        let p = at(&app, 2);
        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        whole_frame(&ctx, &mut app, vec![]);
        // Read in the frame: it is cleared at the next frame's start.
        assert_eq!(app.history_pane.hover, vec![card]);

        let p = at(&app, 3);
        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(p)]);
        whole_frame(&ctx, &mut app, vec![]);
        assert!(
            app.history_pane.hover.is_empty(),
            "the page itself draws nothing"
        );
        let rows = app.history_rows();
        assert_eq!(
            rows[3].page, None,
            "a page's own step does not name it twice"
        );
    }

    /// The divider drags History taller than its list, up to the Layers header and
    /// no further; folded, the section is only its header.
    /// flip: `history_divider`'s clamp loosened → fails at the stored height; the
    /// layers tree's `min_scrolled_height(0.0)` removed → fails at the tree's viewport.
    #[test]
    fn the_divider_stops_at_the_layers_header() {
        let ctx = egui::Context::default();
        let (mut app, _) = shown(&ctx);
        for _ in 0..3 {
            whole_frame(&ctx, &mut app, vec![]);
        }
        let divider = |ctx: &egui::Context| {
            crate::ui::widget_rect(ctx, egui::Id::new("history-divider"))
                .expect("the divider was drawn")
                .rect
        };
        let from = divider(&ctx).center();
        let to = egui::pos2(from.x, 0.0);
        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(from)]);
        whole_frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerButton {
                pos: from,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
        );
        for y in [from.y - 10.0, from.y - 200.0, 0.0] {
            whole_frame(
                &ctx,
                &mut app,
                vec![egui::Event::PointerMoved(egui::pos2(from.x, y))],
            );
        }
        whole_frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        whole_frame(&ctx, &mut app, vec![]);
        let top = divider(&ctx).center().y;
        assert!(
            top < from.y - 200.0,
            "taller than its list: {top} from {}",
            from.y
        );
        assert!(
            (top - app.history_pane.head_bottom).abs() <= 1.0,
            "stopped at the Layers header: {top} against {}",
            app.history_pane.head_bottom
        );
        // The drawn height is clamped again, so the stored one is what proves the
        // drag stopped: past the cap, dragging back down would move nothing at first.
        let stored = app.prefs.history_height.expect("the drag stored a height");
        assert!(
            stored <= 820.0 - app.history_pane.head_bottom + 1.0,
            "the stored height overshot the room: {stored}"
        );
        // Layers keeps its header and nothing of its tree spills over History.
        let tree = app.history_pane.tree_view.expect("the tree was drawn");
        assert!(
            tree.bottom() <= top + 0.5,
            "the tree's viewport runs to {} over History's divider at {top}",
            tree.bottom()
        );

        app.prefs.history_folded = true;
        whole_frame(&ctx, &mut app, vec![]);
        whole_frame(&ctx, &mut app, vec![]);
        let hairline = egui::Id::new("history-divider");
        assert!(
            crate::ui::widget_rect(&ctx, hairline).is_none(),
            "folded, the divider does not drag"
        );
    }
}
