//! Pages (`ondin_core::page`): the Pages section heading the layers pane, the page
//! verbs it shares with the page menu, the keyboard and the palette, and what the
//! app drops when the current page changes.
//!
//! Switching pages is a view change (`EditorSession::switch_page`): no history
//! step, nothing saved in the file. Every door goes through
//! [`OndinApp::go_to_page`], and a switch the session makes on its own (an undo
//! following its step) is noticed by [`OndinApp::notice_page_change`].

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::theme::{self, color, icon};
use crate::ui::{FOLD_HEAD_H, baseline_in, fold_head, icon_button};
use eframe::egui;
use ondin_core::{NodeId, Operation, Transaction, build};

/// A page row's height — the layer tree's.
const ROW_H: f32 = 26.0;
/// How many rows the section shows before it scrolls, until its divider is dragged.
const FIT_ROWS: usize = 6;
/// The least the list shrinks to when its divider is dragged: one row.
const MIN_LIST_H: f32 = ROW_H;
/// The divider's grab band, centred on its hairline.
const DIVIDER_GRAB: f32 = 6.0;
/// The check column's centre, from the row's left.
const CHECK_X: f32 = 14.0;
/// Where a row's name starts, from the row's left.
const NAME_X: f32 = 28.0;

/// The Pages section's own state.
#[derive(Default)]
pub(crate) struct PagesState {
    /// The page the last frame showed. A change by any door — an undo following
    /// its step included — runs [`OndinApp::page_changed`].
    pub(crate) shown: Option<NodeId>,
    /// A row being renamed in place.
    renaming: Option<Renaming>,
    /// A row being dragged to reorder the pages, and the slot it would drop into.
    drag: Option<(NodeId, Option<usize>)>,
    /// A page with layers waiting on *Delete page*'s confirmation.
    pub(crate) deleting: Option<NodeId>,
    /// Where the `+` was drawn, for tests that press it.
    #[cfg(test)]
    plus: Option<egui::Rect>,
}

/// A page row's rename in flight.
struct Renaming {
    page: NodeId,
    text: String,
    /// The undo step the name folds into: set when the page was just added, so
    /// adding and naming it is one step (`EditorSession::commit_into_last`).
    fold: Option<(usize, u64)>,
    /// Whether the field has asked for focus yet — once, on its first frame.
    focused: bool,
}

impl OndinApp {
    /// A page's name, as its root node carries it.
    pub(crate) fn page_name(&self, page: NodeId) -> String {
        self.session
            .doc
            .get(page)
            .map_or_else(String::new, |n| n.name().to_string())
    }

    /// Make `page` current — every door's one path. A live text edit is finished
    /// first, since it belongs to the page being left.
    pub(crate) fn go_to_page(&mut self, page: NodeId) {
        if page == self.session.page || !self.session.doc.is_page(page) {
            return;
        }
        self.finish_text_first();
        // The page being left counts as shown, even before a frame noticed it, or
        // the switch would read as the document's first frame.
        self.pages.shown.get_or_insert(self.session.page);
        self.session.switch_page(page);
        self.page_changed();
    }

    /// Run [`Self::page_changed`] when the session switched pages behind the
    /// app's back — an undo or redo following its step. Called once a frame.
    pub(crate) fn notice_page_change(&mut self) {
        if self.pages.shown != Some(self.session.page) {
            self.page_changed();
        }
    }

    /// Put a just-opened document back on the page it was last left on here —
    /// the library's record, never the file's.
    pub(crate) fn reopen_last_page(&mut self) {
        self.pages = PagesState::default();
        let Some(id) = self.session.doc.meta().id.clone() else {
            return;
        };
        let page = self
            .library
            .local
            .last_page
            .get(&id)
            .and_then(|w| NodeId::from_wire(w));
        if let Some(page) = page {
            self.session.switch_page(page);
        }
    }

    /// The current page just changed: drop the gesture and panel state naming
    /// the other page's layers, frame a page shown for the first time, and let
    /// the library remember where the document was left.
    pub(crate) fn page_changed(&mut self) {
        let first = self.pages.shown.is_none();
        self.pages.shown = Some(self.session.page);
        if first {
            // The document's first frame: nothing of another page to drop.
            self.session.take_first_visit();
            return;
        }
        if let Some(id) = self.session.doc.meta().id.clone() {
            self.library
                .local
                .last_page
                .insert(id, self.session.page.to_wire());
            self.library.local.save();
        }
        self.drag = Drag::None;
        self.alt_clone = None;
        self.pen = None;
        self.points.clear();
        self.name_edit = None;
        self.desc_edit = None;
        self.renaming_layer = None;
        self.layer_drag = None;
        self.layers_range_anchor = None;
        self.entered_group = None;
        self.picker = None;
        self.guide_previews.clear();
        if self.session.take_first_visit() {
            match self.document_bounds() {
                Some(b) => self.zoom_to(Some(b)),
                None => self.session.camera = crate::view::Camera::default(),
            }
        }
    }

    /// Step `by` pages along the order — `Ctrl+PageDown` is `+1` — stopping at
    /// either end.
    pub(crate) fn step_page(&mut self, by: isize) {
        let doc = &self.session.doc;
        let Some(at) = doc.page_index(self.session.page) else {
            return;
        };
        let to = at.saturating_add_signed(by).min(doc.pages().len() - 1);
        let page = doc.pages()[to].id;
        self.go_to_page(page);
    }

    /// *New page*: `Page N` at the end of the order, on the current page's
    /// ground, made current with its name open for typing. Adding it and naming it
    /// are one undo step.
    pub(crate) fn add_page(&mut self) {
        self.finish_text_first();
        let doc = &self.session.doc;
        let taken: Vec<String> = doc.page_ids().map(|p| self.page_name(p)).collect();
        let name = ondin_core::page::free_name(taken.iter().map(String::as_str));
        let id = self.session.ids.mint();
        let tx = Transaction(vec![Operation::AddPage {
            id,
            index: doc.pages().len(),
            name,
            ground: doc.ground(self.session.page),
            draft: false,
            mode: None,
        }]);
        if !self.session.commit(tx) {
            return;
        }
        self.go_to_page(id);
        self.start_new_page_rename(id);
        self.session.info("Added a page");
    }

    /// Open `page`'s row for renaming.
    pub(crate) fn start_page_rename(&mut self, page: NodeId) {
        self.pages.renaming = Some(Renaming {
            page,
            text: self.page_name(page),
            fold: None,
            focused: false,
        });
        self.pages_unfold();
    }

    /// Open the row of `page`, made by the last commit, for renaming into that
    /// commit's undo step.
    fn start_new_page_rename(&mut self, page: NodeId) {
        self.start_page_rename(page);
        if let Some(r) = &mut self.pages.renaming {
            r.fold = Some((self.session.history.undo_depth(), self.session.commits()));
        }
    }

    /// Commit a rename. An empty name or the name it has changes nothing.
    fn commit_page_name(&mut self, page: NodeId, name: &str, fold: Option<(usize, u64)>) {
        let name = name.trim();
        if name.is_empty() || name == self.page_name(page) {
            return;
        }
        let tx = Transaction(vec![Operation::SetName {
            id: page,
            name: name.to_string(),
        }]);
        match fold {
            Some(at) => self.session.commit_into_last(tx, at),
            None => self.session.commit(tx),
        };
    }

    /// *Duplicate page*: a copy of `page` directly below it, named `… copy`, its
    /// layers copied the way *Duplicate* copies them and its guides with them,
    /// made current with its name open for typing. Duplicating and naming it are
    /// one undo step.
    pub(crate) fn duplicate_page(&mut self, page: NodeId) {
        self.finish_text_first();
        let doc = &self.session.doc;
        let (Some(index), Some(info), Some(root)) =
            (doc.page_index(page), doc.page(page), doc.get(page))
        else {
            return;
        };
        let id = self.session.ids.mint();
        let taken: Vec<String> = doc.page_ids().map(|p| self.page_name(p)).collect();
        // `Pricing copy`, then `Pricing copy 2` when that is taken.
        let stem = format!("{} copy", root.name());
        let name = std::iter::once(stem.clone())
            .chain((2..).map(|n| format!("{stem} {n}")))
            .find(|n| !taken.contains(n))
            .expect("an unbounded range finds a free name");
        let mut ops = vec![Operation::AddPage {
            id,
            index: index + 1,
            name,
            ground: info.ground,
            draft: info.draft,
            mode: root.mode(),
        }];
        let templates: Vec<Vec<ondin_core::Node>> = root
            .children()
            .iter()
            .filter_map(|c| doc.capture_subtree(*c))
            .collect();
        let placements: Vec<build::Placement> = templates
            .iter()
            .enumerate()
            .map(|(i, t)| build::Placement {
                nodes: t.clone(),
                parent: id,
                index: Some(i),
            })
            .collect();
        let (copies, _) =
            build::insert_subtrees(doc, &mut self.session.ids, &placements, Default::default());
        // Old id → new, read off the inserts, which keep each template's order.
        let mut map = std::collections::HashMap::new();
        map.insert(page, id);
        let inserted = copies.0.iter().filter_map(|op| match op {
            Operation::InsertSubtree { nodes, .. } => Some(nodes),
            _ => None,
        });
        for (template, nodes) in templates.iter().zip(inserted) {
            for (from, to) in template.iter().zip(nodes) {
                map.insert(from.id(), to.id());
            }
        }
        ops.extend(copies.0);
        let pages_guides: Vec<ondin_core::Guide> = doc
            .guides()
            .iter()
            .filter(|g| doc.page_of(g.owner) == Some(page))
            .copied()
            .collect();
        for g in pages_guides {
            if let Some(owner) = map.get(&g.owner) {
                ops.push(Operation::AddGuide {
                    guide: ondin_core::Guide {
                        id: ondin_core::GuideId(self.session.ids.mint()),
                        owner: *owner,
                        ..g
                    },
                });
            }
        }
        if self.session.commit(Transaction(ops)) {
            self.go_to_page(id);
            self.start_new_page_rename(id);
            self.session.info("Duplicated the page");
        }
    }

    /// *Delete page*: an empty page goes at once; one with layers asks first
    /// ([`Self::page_delete_confirmation`]).
    pub(crate) fn delete_page(&mut self, page: NodeId) {
        let doc = &self.session.doc;
        let has_layers = doc.get(page).is_some_and(|n| !n.children().is_empty());
        if has_layers && doc.pages().len() > 1 {
            self.pages.deleting = Some(page);
        } else {
            self.delete_page_now(page);
        }
    }

    /// *Delete page?*: what goes, then *Cancel* and *Delete*. `Escape`, the
    /// backdrop and the ✕ cancel.
    pub(crate) fn page_delete_confirmation(&mut self, ctx: &egui::Context) {
        let Some(page) = self.pages.deleting else {
            return;
        };
        let Some(layers) = self.session.doc.get(page).map(|n| n.children().len()) else {
            self.pages.deleting = None;
            return;
        };
        let name = self.page_name(page);
        let body = format!(
            "“{name}” and its {} go with it. Ctrl+Z brings them back.",
            match layers {
                1 => "1 layer".to_string(),
                n => format!("{n} layers"),
            }
        );
        let (mut delete, mut cancel) = (false, false);
        let modal = crate::settings::card("confirm-page-delete", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(320.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, "Delete page?") {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(body)
                        .size(12.0)
                        .color(theme::text::MUTED),
                )
                .wrap(),
            );
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Cancel", crate::ui::FieldButton::Off).clicked()
                {
                    cancel = true;
                }
                if crate::settings::text_button(ui, "Delete", crate::ui::FieldButton::Danger)
                    .clicked()
                {
                    delete = true;
                }
            });
        });
        if delete {
            self.pages.deleting = None;
            self.delete_page_now(page);
        } else if cancel || modal.should_close() {
            self.pages.deleting = None;
        }
    }

    /// Delete `page`: its guides, then its layers — a main among them detaching its
    /// instances on other pages, as *Delete* does — then the page. Leaving the
    /// current page lands on the one above it. One undo step; the last page stays.
    pub(crate) fn delete_page_now(&mut self, page: NodeId) {
        self.finish_text_first();
        let doc = &self.session.doc;
        let Some(index) = doc.page_index(page) else {
            return;
        };
        if doc.pages().len() == 1 {
            self.session.info("A document keeps at least one page");
            return;
        }
        let name = self.page_name(page);
        let layers: Vec<NodeId> = doc
            .get(page)
            .map(|n| n.children().to_vec())
            .unwrap_or_default();
        let mut ops: Vec<Operation> = doc
            .guides()
            .iter()
            .filter(|g| g.owner == page)
            .map(|g| Operation::RemoveGuide { id: g.id })
            .collect();
        ops.extend(build::guides_of(doc, &layers));
        ops.extend(layers.iter().map(|id| Operation::DeleteNode { id: *id }));
        let relinks = ondin_core::component::relink_for_delete(doc, &layers);
        let detached = relinks
            .iter()
            .filter(|op| {
                matches!(op, Operation::SetLink { id, link: None }
                    if ondin_core::component::instance_root(doc, *id) == Some(*id))
            })
            .count();
        ops.extend(relinks);
        ops.push(Operation::RemovePage { id: page });
        // The page above, or the one below for the first.
        let above = doc.pages()[index.saturating_sub(1).max(usize::from(index == 0))].id;
        let leaving = page == self.session.page;
        if self.session.commit(Transaction(ops)) {
            if leaving {
                self.go_to_page(above);
            }
            self.session.info(match detached {
                0 => format!("Deleted page “{name}” · Ctrl+Z to undo"),
                1 => format!("Deleted page “{name}” — 1 instance detached · Ctrl+Z to undo"),
                n => format!("Deleted page “{name}” — {n} instances detached · Ctrl+Z to undo"),
            });
        }
    }

    /// Mark `page` as a draft, or clear it.
    pub(crate) fn toggle_page_draft(&mut self, page: NodeId) {
        let Some(draft) = self.session.doc.page(page).map(|p| p.draft) else {
            return;
        };
        if self
            .session
            .commit(Transaction(vec![Operation::SetPageDraft {
                id: page,
                draft: !draft,
            }]))
        {
            let name = self.page_name(page);
            self.session.info(match draft {
                true => format!("“{name}” is no longer a draft"),
                false => {
                    format!("“{name}” is a draft: left out of publishing, export all and present")
                }
            });
        }
    }

    /// *Move to page*: the selection's outermost layers to the top of `page`, in
    /// their order, at the same place on the canvas. They leave the selection and
    /// the view stays. One undo step.
    pub(crate) fn move_selection_to_page(&mut self, page: NodeId) {
        self.finish_text_first();
        let doc = &self.session.doc;
        let ids = build::in_document_order(doc, self.session.selection.ids());
        if ids.is_empty() || !doc.is_page(page) || page == self.session.page {
            return;
        }
        let end = doc.get(page).map_or(0, |n| n.children().len());
        let mut ops = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            match build::reparent_preserving_world(doc, &self.session.resolved, *id, page, end + i)
            {
                Ok(tx) => ops.extend(tx.0),
                Err(e) => {
                    self.session.fail(format!("Could not move the layers: {e}"));
                    return;
                }
            }
        }
        let n = ids.len();
        let name = self.page_name(page);
        if self.session.commit(Transaction(ops)) {
            self.session.selection.clear();
            self.session.info(match n {
                1 => format!("Moved 1 layer to {name} · Ctrl+Z to undo"),
                n => format!("Moved {n} layers to {name} · Ctrl+Z to undo"),
            });
        }
    }

    /// Open the section if it was folded, for a door that needs its rows.
    fn pages_unfold(&mut self) {
        if self.prefs.pages_folded {
            self.prefs.pages_folded = false;
            self.prefs.save();
        }
    }

    /// The Pages section: header, the rows, and the divider above *Layers*.
    pub(crate) fn pages_section(&mut self, ui: &mut egui::Ui) {
        let folded = self.prefs.pages_folded;
        egui::Frame::NONE
            .inner_margin(egui::Margin {
                left: crate::panels::layers::PAD_X,
                right: crate::panels::layers::PAD_X,
                top: 14,
                // Folded, as far above the divider as the Layers header sits below it.
                bottom: if folded { 9 } else { 7 },
            })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (head, baseline) = fold_head(ui, "Pages", !folded);
                    if head.clicked() {
                        self.prefs.pages_folded = !folded;
                        self.prefs.save();
                    }
                    if folded {
                        // On the eyebrow's baseline, which centring the two would miss.
                        ui.add_space(4.0);
                        // Clear of the `+` and its gap.
                        let room = ui.available_width() - 20.0 - 2.0 * ui.spacing().item_spacing.x;
                        let mut job = egui::text::LayoutJob::simple_singleline(
                            self.page_name(self.session.page),
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
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let plus = icon_button(ui, icon::PLUS, 20.0, 15.0, false, true);
                        #[cfg(test)]
                        {
                            self.pages.plus = Some(plus.rect);
                        }
                        if plus.on_hover_text("New page").clicked() {
                            self.add_page();
                        }
                    });
                });
            });
        if !folded {
            let fit = self.session.doc.pages().len().min(FIT_ROWS) as f32 * ROW_H;
            let height = self.prefs.pages_height.unwrap_or(fit).max(MIN_LIST_H);
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: crate::panels::layers::TREE_PAD_X,
                    right: crate::panels::layers::TREE_PAD_X,
                    top: 0,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("pages")
                        .max_height(height)
                        .min_scrolled_height(height.min(fit))
                        .auto_shrink([false, true])
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| self.page_rows(ui));
                });
        }
        self.pages_divider(ui, folded);
    }

    /// The divider under the section: a hairline, and — while the section is open —
    /// a band that drags the list's height, kept as a preference.
    fn pages_divider(&mut self, ui: &mut egui::Ui, folded: bool) {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0.0, color::DIVIDER);
        if folded {
            return;
        }
        let band = rect.expand2(egui::vec2(0.0, DIVIDER_GRAB / 2.0));
        let resp = ui.interact(band, egui::Id::new("pages-divider"), egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if resp.dragged() {
            let fit = self.session.doc.pages().len().min(FIT_ROWS) as f32 * ROW_H;
            let now = self.prefs.pages_height.unwrap_or(fit);
            let all = self.session.doc.pages().len() as f32 * ROW_H;
            self.prefs.pages_height =
                Some((now + resp.drag_delta().y).clamp(MIN_LIST_H, all.max(MIN_LIST_H)));
        }
        if resp.drag_stopped() {
            self.prefs.save();
        }
    }

    /// One row per page, in order.
    fn page_rows(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let pages: Vec<(NodeId, bool)> = self
            .session
            .doc
            .pages()
            .iter()
            .map(|p| (p.id, p.draft))
            .collect();
        let pointer = ui.ctx().pointer_interact_pos();
        let released = ui.input(|i| i.pointer.any_released());
        let mut rects = Vec::with_capacity(pages.len());
        for (page, draft) in &pages {
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ROW_H),
                egui::Sense::click_and_drag(),
            );
            rects.push(rect);
            let current = *page == self.session.page;
            let renaming = self
                .pages
                .renaming
                .as_ref()
                .is_some_and(|r| r.page == *page);
            // A layer drag lights the page it would land on (`move_selection_to_page`);
            // the current page takes nothing.
            let dropping =
                self.layer_drag.is_some() && !current && pointer.is_some_and(|p| rect.contains(p));
            let dragged = self.pages.drag.is_some_and(|(p, _)| p == *page);
            let p = ui.painter();
            let r5 = egui::CornerRadius::same(5);
            if current {
                p.rect_filled(rect, r5, theme::color::text_a(20));
            } else if resp.hovered() && self.layer_drag.is_none() {
                p.rect_filled(rect, r5, theme::color::text_a(12));
            }
            if dropping {
                p.rect_stroke(
                    rect.shrink(0.75),
                    r5,
                    egui::Stroke::new(1.5, color::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
            let cy = rect.center().y;
            if current {
                p.text(
                    egui::pos2(rect.left() + CHECK_X, cy),
                    egui::Align2::CENTER_CENTER,
                    icon::CHECK,
                    theme::icon_font(13.0),
                    color::ACCENT,
                );
            }
            let ink = match (dragged, *draft, current) {
                (true, ..) => theme::text::FAINT,
                (_, true, _) => theme::text::DIM,
                (_, _, true) => color::TEXT,
                _ => theme::text::MUTED,
            };
            if !renaming {
                let name = self.page_name(*page);
                p.text(
                    egui::pos2(rect.left() + NAME_X, cy),
                    egui::Align2::LEFT_CENTER,
                    name,
                    egui::FontId::proportional(12.5),
                    ink,
                );
            }
            if *draft {
                let tag = p.layout_no_wrap(
                    "Draft".to_string(),
                    egui::FontId::proportional(10.0),
                    theme::text::DIM,
                );
                let size = tag.size() + egui::vec2(10.0, 3.0);
                let at = egui::Rect::from_min_size(
                    egui::pos2(rect.right() - 6.0 - size.x, cy - size.y / 2.0),
                    size,
                );
                p.rect_stroke(
                    at,
                    egui::CornerRadius::same(3),
                    egui::Stroke::new(1.0, theme::color::text_a(40)),
                    egui::StrokeKind::Inside,
                );
                p.galley(at.min + egui::vec2(5.0, 1.5), tag, theme::text::DIM);
            }
            if renaming {
                self.page_rename_field(ui, *page, rect);
                continue;
            }
            if resp.secondary_clicked() {
                self.open_context_menu(ui.ctx(), crate::menu::Target::Page(*page), None);
            }
            if crate::ui::double_clicked(ui, &resp) {
                self.start_page_rename(*page);
            } else if resp.clicked() && self.context_menu.is_none() {
                self.go_to_page(*page);
            }
            if resp.drag_started() {
                self.pages.drag = Some((*page, None));
            }
            if dropping && released {
                self.move_selection_to_page(*page);
                self.layer_drag = None;
            }
        }
        self.page_drag_ui(ui, &pages, &rects);
    }

    /// A page row in flight: the slot it would drop into, drawn as the layer
    /// tree's insertion line, and the move on release.
    fn page_drag_ui(&mut self, ui: &mut egui::Ui, pages: &[(NodeId, bool)], rects: &[egui::Rect]) {
        let Some((page, _)) = self.pages.drag else {
            return;
        };
        let Some(y) = ui.ctx().pointer_interact_pos().map(|p| p.y) else {
            return;
        };
        // The gap the pointer is nearest: before row `i`, or after the last.
        let slot = rects
            .iter()
            .position(|r| y < r.center().y)
            .unwrap_or(rects.len());
        self.pages.drag = Some((page, Some(slot)));
        if let (Some(first), Some(last)) = (rects.first(), rects.last()) {
            let line_y = rects.get(slot).map_or(last.bottom(), |r| r.top());
            let x = first.left() + 4.0..=first.right() - 4.0;
            ui.painter()
                .hline(x, line_y, egui::Stroke::new(2.0, color::ACCENT));
        }
        if ui.input(|i| i.pointer.any_released()) {
            self.pages.drag = None;
            let Some(from) = pages.iter().position(|(p, _)| *p == page) else {
                return;
            };
            // Removing the row first shifts every slot below it up one.
            let to = if slot > from { slot - 1 } else { slot };
            if to != from {
                self.session.commit(Transaction(vec![Operation::MovePage {
                    id: page,
                    index: to,
                }]));
            }
        }
    }

    /// The rename field over `page`'s row. Enter keeps the name; Escape keeps the
    /// old one (for a page just added, its default); a press elsewhere keeps the
    /// typed one.
    fn page_rename_field(&mut self, ui: &mut egui::Ui, page: NodeId, row: egui::Rect) {
        let Some(r) = self.pages.renaming.as_mut() else {
            return;
        };
        let id = egui::Id::new(("page-rename", page));
        let field = egui::Rect::from_min_max(
            egui::pos2(row.left() + NAME_X - 4.0, row.top() + 3.0),
            egui::pos2(row.right() - 6.0, row.bottom() - 3.0),
        );
        ui.painter().rect(
            field,
            egui::CornerRadius::same(4),
            color::FIELD,
            egui::Stroke::new(1.0, color::ACCENT),
            egui::StrokeKind::Inside,
        );
        // `place`, not `put`: the field is shorter than its row, and `put` would
        // pull the rows below up to the field's bottom.
        let resp = ui.place(
            field.shrink2(egui::vec2(4.0, 0.0)),
            egui::TextEdit::singleline(&mut r.text)
                .id(id)
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                .vertical_align(egui::Align::Center)
                .desired_width(field.width() - 8.0)
                .font(egui::FontId::proportional(12.5))
                .text_color(color::TEXT),
        );
        // Focus once, on the field's first frame (the layer rename's rule).
        if !r.focused {
            r.focused = true;
            resp.request_focus();
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(r.text.chars().count()),
            );
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), id);
            }
        }
        if resp.has_focus()
            && ui.input(|i| i.pointer.any_pressed())
            && ui
                .ctx()
                .pointer_interact_pos()
                .is_none_or(|p| !field.contains(p))
        {
            ui.ctx().memory_mut(|m| m.surrender_focus(id));
        }
        if resp.lost_focus() {
            let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let Some(r) = self.pages.renaming.take() else {
                return;
            };
            if !cancelled {
                self.commit_page_name(page, &r.text, r.fold);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{Document, IdSource, NodeKind};

    /// Two pages, `Page 1` and `Pricing`, a frame on each; the app on `Page 1`.
    fn two_pages(ctx: &egui::Context) -> (OndinApp, [NodeId; 4]) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xA9E5);
        let first = ids.mint();
        let mut doc = Document::new(first);
        let (second, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let frame = |id, parent, x: f64| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: None,
        };
        doc.apply(&Transaction(vec![
            Operation::AddPage {
                id: second,
                index: 1,
                name: "Pricing".into(),
                ground: ondin_core::DEFAULT_CANVAS_BACKGROUND,
                draft: false,
                mode: None,
            },
            frame(a, first, 0.0),
            frame(b, second, 500.0),
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        (app, [first, second, a, b])
    }

    /// Each page keeps its own camera and selection for the sitting; a page shown
    /// for the first time is framed on its contents.
    ///
    /// flip: `switch_page` without its `parked.insert` → fails at "the first
    /// page's selection comes back".
    #[test]
    fn a_page_comes_back_as_it_was_left() {
        let ctx = egui::Context::default();
        let (mut app, [first, second, a, b]) = two_pages(&ctx);
        app.canvas_px = (800, 600);
        app.session.selection.set(vec![a]);
        app.session.camera.center = Point::new(-300.0, 7.0);
        app.go_to_page(second);
        assert_eq!(app.session.page, second);
        assert!(
            app.session.selection.is_empty(),
            "a page's selection is its own"
        );
        assert_eq!(
            app.session.camera.center,
            Point::new(550.0, 50.0),
            "framed on the frame at 500"
        );
        app.session.selection.set(vec![b]);
        app.go_to_page(first);
        assert_eq!(
            app.session.selection.ids(),
            &[a],
            "the first page's selection comes back"
        );
        assert_eq!(app.session.camera.center, Point::new(-300.0, 7.0));
        app.go_to_page(second);
        assert_eq!(app.session.selection.ids(), &[b]);
    }

    /// *Move to page* keeps the layer where it was on the canvas, clears it from
    /// the selection and leaves the view; undoing it from the other page goes
    /// back to where the layer is again and selects it.
    #[test]
    fn a_moved_layer_keeps_its_place_and_undo_follows_it() {
        let ctx = egui::Context::default();
        let (mut app, [first, second, a, _]) = two_pages(&ctx);
        let world = app.session.resolved.world_transform(a);
        app.session.selection.set(vec![a]);
        app.move_selection_to_page(second);
        assert_eq!(app.session.doc.page_of(a), Some(second));
        assert_eq!(app.session.resolved.world_transform(a), world, "same place");
        assert_eq!(app.session.page, first, "the view stays");
        assert!(app.session.selection.is_empty());

        app.go_to_page(second);
        assert!(app.session.undo());
        assert_eq!(
            app.session.page, first,
            "the undo went where the layer went back to"
        );
        assert_eq!(app.session.selection.ids(), &[a], "and selected it");
    }

    /// Adding a page and naming it is one undo step.
    ///
    /// flip: `commit_page_name` ignoring its `fold` → fails at "one step takes
    /// both away".
    #[test]
    fn adding_and_naming_a_page_is_one_step() {
        let ctx = egui::Context::default();
        let (mut app, _) = two_pages(&ctx);
        app.add_page();
        let added = app.session.page;
        assert_eq!(app.page_name(added), "Page 2", "the lowest free number");
        let fold = app.pages.renaming.as_ref().and_then(|r| r.fold);
        app.pages.renaming = None;
        app.commit_page_name(added, "Archive", fold);
        assert_eq!(app.page_name(added), "Archive");
        assert!(app.session.undo());
        assert!(!app.session.doc.is_page(added), "one step takes both away");
        app.notice_page_change();
        assert!(app.session.doc.is_page(app.session.page));
    }

    /// Deleting a page with a main on it detaches the instance on the other page,
    /// lands on the page above, and undoes whole.
    #[test]
    fn deleting_a_page_detaches_its_mains_instances_elsewhere() {
        let ctx = egui::Context::default();
        let (mut app, [first, second, a, b]) = two_pages(&ctx);
        app.session
            .commit(Transaction(vec![Operation::SetComponent {
                id: b,
                component: true,
            }]));
        let template = app.session.doc.capture_subtree(b).unwrap();
        let (tx, made) = build::insert_subtrees(
            &app.session.doc,
            &mut app.session.ids,
            &[build::Placement {
                nodes: template,
                parent: a,
                index: None,
            }],
            Default::default(),
        );
        assert!(app.session.commit(tx));
        let instance = made[0];
        assert!(
            app.session.doc.get(instance).unwrap().link().is_some(),
            "fixture"
        );
        let before = app.session.doc.clone();

        app.go_to_page(second);
        app.delete_page_now(second);
        assert!(!app.session.doc.is_page(second));
        assert_eq!(app.session.page, first, "the page above");
        assert!(
            app.session.doc.get(instance).unwrap().link().is_none(),
            "the other page's instance is detached"
        );
        assert!(app.session.undo());
        assert_eq!(app.session.doc, before, "undone whole");
    }

    /// *Duplicate page* copies its layers and its guides onto a page `… copy`
    /// below it.
    #[test]
    fn a_duplicated_page_carries_its_layers_and_guides() {
        let ctx = egui::Context::default();
        let (mut app, [_, second, _, b]) = two_pages(&ctx);
        let (on_page, on_frame) = (
            ondin_core::GuideId(app.session.ids.mint()),
            ondin_core::GuideId(app.session.ids.mint()),
        );
        let guide = |id, owner| ondin_core::Guide {
            id,
            axis: ondin_core::GuideAxis::Vertical,
            position: 3.0,
            color: None,
            owner,
        };
        app.session.commit(Transaction(vec![
            Operation::AddGuide {
                guide: guide(on_page, second),
            },
            Operation::AddGuide {
                guide: guide(on_frame, b),
            },
        ]));
        app.duplicate_page(second);
        let copy = app.session.page;
        assert_eq!(app.page_name(copy), "Pricing copy");
        assert_eq!(app.session.doc.page_index(copy), Some(2));
        let frames = app.session.doc.get(copy).unwrap().children().to_vec();
        assert_eq!(frames.len(), 1);
        assert_ne!(frames[0], b, "fresh ids");
        let owners: Vec<NodeId> = app
            .session
            .doc
            .guides()
            .iter()
            .filter(|g| app.session.doc.page_of(g.owner) == Some(copy))
            .map(|g| g.owner)
            .collect();
        assert_eq!(owners.len(), 2, "both guides came along: {owners:?}");
        assert!(owners.contains(&copy) && owners.contains(&frames[0]));
    }

    /// `Ctrl+PageDown` steps along the order and stops at the last page.
    #[test]
    fn stepping_pages_stops_at_the_ends() {
        let ctx = egui::Context::default();
        let (mut app, [first, second, ..]) = two_pages(&ctx);
        app.step_page(1);
        assert_eq!(app.session.page, second);
        app.step_page(1);
        assert_eq!(app.session.page, second, "no wrap");
        app.step_page(-1);
        app.step_page(-1);
        assert_eq!(app.session.page, first);
    }

    /// One whole app frame, 1320 × 820.
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

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// The page the `+` adds arrives with its name field focused, and `Escape`
    /// closes the field — through whole frames, since what took the focus was a
    /// panel drawn after the section.
    ///
    /// flip: `inspector_panel` reading the focused widget by `read_response` →
    /// fails at "arrives focused".
    #[test]
    fn a_page_added_by_its_button_arrives_focused() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let (mut app, _) = two_pages(&ctx);
        app.view = crate::app::View::Editor;
        for _ in 0..3 {
            whole_frame(&ctx, &mut app, vec![]);
        }
        let at = app.pages.plus.expect("the + was drawn").center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        whole_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        whole_frame(&ctx, &mut app, vec![button(true)]);
        whole_frame(&ctx, &mut app, vec![button(false)]);
        let field = egui::Id::new(("page-rename", app.session.page));
        assert_eq!(
            app.session.doc.pages().len(),
            3,
            "fixture: the + added a page"
        );
        whole_frame(&ctx, &mut app, vec![]);
        assert_eq!(ctx.memory(|m| m.focused()), Some(field), "arrives focused");

        whole_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        whole_frame(&ctx, &mut app, vec![]);
        assert!(app.pages.renaming.is_none(), "Escape closes the field");
    }

    /// The Pages section stands still while a row is renamed: the divider under
    /// it stays put.
    ///
    /// flip: the field laid with `ui.put` → fails at "nothing below moves".
    #[test]
    fn renaming_a_page_moves_nothing_below_it() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let (mut app, [first, ..]) = two_pages(&ctx);
        let divider = |app: &mut OndinApp| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(300.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| app.pages_section(ui),
            );
            crate::ui::widget_rect(&ctx, egui::Id::new("pages-divider"))
                .expect("the divider was drawn")
                .rect
        };
        divider(&mut app);
        let before = divider(&mut app);
        app.start_page_rename(first);
        // The scroll area is sized by the frame before's content.
        divider(&mut app);
        let during = divider(&mut app);
        assert!(app.pages.renaming.is_some(), "fixture: the field is open");
        assert_eq!(during, before, "nothing below moves");
    }

    /// *Delete page* asks first when the page has layers, and deletes an empty
    /// one at once.
    #[test]
    fn deleting_a_page_with_layers_asks_first() {
        let ctx = egui::Context::default();
        let (mut app, [_, second, ..]) = two_pages(&ctx);
        app.delete_page(second);
        assert!(app.session.doc.is_page(second), "not yet");
        assert_eq!(app.pages.deleting, Some(second));
        assert!(app.modal_is_up());
        app.pages.deleting = None;

        app.add_page();
        let empty = app.session.page;
        app.pages.renaming = None;
        app.delete_page(empty);
        assert!(
            !app.session.doc.is_page(empty),
            "an empty page goes at once"
        );
        assert!(app.pages.deleting.is_none());
    }

    /// *Duplicate page* opens the copy's name for typing, and duplicating and
    /// naming it are one undo step.
    #[test]
    fn a_duplicated_page_is_named_in_the_same_step() {
        let ctx = egui::Context::default();
        let (mut app, [_, second, ..]) = two_pages(&ctx);
        app.duplicate_page(second);
        let copy = app.session.page;
        let r = app.pages.renaming.take().expect("the copy's name is open");
        assert_eq!(r.page, copy);
        app.commit_page_name(copy, "Pricing v2", r.fold);
        assert_eq!(app.page_name(copy), "Pricing v2");
        assert!(app.session.undo());
        assert!(!app.session.doc.is_page(copy), "one step takes both away");
    }
}
