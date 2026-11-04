//! Design tokens import (`docs/design/styles-variables.md`, *Design tokens*): a
//! DTCG `tokens.json` read back into the document's variables and styles, once or
//! linked.
//!
//! The reading and the comparison are `ondin_export::dtcg_in`'s; this module
//! opens the file, draws the review — one row per change, a box each — and
//! commits the ticked rows as one undo step, *Import tokens*. A linked file is
//! watched as a kit's file is, and its pill opens the review of only what the
//! file changed since it was last applied.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::meta::TokensLink;
use ondin_core::{IdSource, Transaction};
use ondin_export::dtcg_in::{self, Change, Review, Row, Shown};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The box's width: a value column per mode needs more than batch rename's.
const W: f32 = 620.0;
const PAD: f32 = 16.0;
/// The list scrolls past this height.
const LIST_H: f32 = 380.0;
const ROW_H: f32 = 26.0;
const ROW_H2: f32 = 38.0;
const GROUP_H: f32 = 24.0;
/// The box and tag columns.
const BOX_W: f32 = 22.0;
const GAP: f32 = 8.0;
/// The widest a mode's value column grows.
const COL_W: f32 = 150.0;

/// How often a linked file is looked at while its document is open — the kits'
/// interval, and on the frame the window regains the focus.
const WATCH_EVERY: f64 = crate::kits::updates::WATCH_EVERY;

/// The review box, while it is open.
pub(crate) struct TokensImport {
    path: PathBuf,
    /// The file as read, kept to record it as applied when linked.
    text: String,
    review: Review,
    ticked: Vec<bool>,
    filter: Option<Change>,
    /// The groups closed by their caret.
    collapsed: HashSet<String>,
    /// *Keep this file linked*.
    link: bool,
    /// Whether the file has a path to link: a browser drag brings only bytes.
    linkable: bool,
    /// Opened by the linked file's pill: its Cancel is *Later*.
    from_link: bool,
    /// Why Apply is off, asked again whenever a box changes.
    why_not: Option<String>,
}

/// A file's modification time in whole seconds and its size.
type Stat = Option<(u64, u64)>;

/// When the linked file was last looked at, and what its look found.
#[derive(Default)]
pub(crate) struct TokensWatch {
    next: f64,
    focused: bool,
    /// What `changes` was counted for: the link's path, the file's stat, and
    /// the applied copy.
    key: Option<(String, Stat, String)>,
    pub(crate) changes: usize,
    /// *Later* on the pill: the file as it was then. A change to it grows the
    /// pill back.
    later: Option<(String, Stat)>,
}

/// A file's [`Stat`]; `None` when it cannot be read.
fn on_disk(path: &Path) -> Stat {
    let m = std::fs::metadata(path).ok()?;
    let modified = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some((modified, m.len()))
}

/// `tokens.json · kestrel-web/design`: the file and the two folders above it.
fn where_label(path: &Path) -> (String, String) {
    let name = path.file_name().map_or_else(
        || "tokens.json".to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let dirs: Vec<String> = path
        .parent()
        .into_iter()
        .flat_map(|p| p.components().rev().take(2))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .filter(|c| !c.is_empty() && c != "\\" && c != "/")
        .collect();
    let dirs: Vec<String> = dirs.into_iter().rev().collect();
    (name, dirs.join("/"))
}

/// What the linked file changed since it was last applied, counted against
/// `doc` with ids that are thrown away.
fn linked_changes(doc: &ondin_core::Document, link: &TokensLink) -> usize {
    let Ok(text) = std::fs::read_to_string(&link.path) else {
        return 0;
    };
    let (Ok(file), Ok(old)) = (
        dtcg_in::parse(&text),
        dtcg_in::parse(&link.applied.to_string()),
    ) else {
        return 0;
    };
    let mut ids = IdSource::new(0x70C3);
    match dtcg_in::review(doc, &file, doc.variables().first_collection().id, &mut ids) {
        Ok(mut r) => {
            r.since(&file, &old);
            r.changes()
        }
        Err(_) => 0,
    }
}

impl OndinApp {
    /// *Import tokens…*: pick a `tokens.json` and open its review.
    pub(crate) fn import_tokens_action(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Design tokens", &["json"])
            .pick_file()
        else {
            return;
        };
        self.open_tokens_import(path, false);
    }

    /// *Export tokens…*: the document's variables and styles as a `tokens.json`,
    /// the file `ondin tokens export` writes.
    pub(crate) fn export_tokens_action(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Design tokens", &["json"])
            .set_file_name("tokens.json")
            .save_file()
        else {
            return;
        };
        let tokens = ondin_export::dtcg::tokens(&self.session.doc, None);
        let (name, _) = where_label(&path);
        match std::fs::write(&path, &tokens.json) {
            Ok(()) => self.session.info(match tokens.notes.len() {
                0 => format!("Exported {name}"),
                1 => format!("Exported {name} \u{b7} {}", tokens.notes[0]),
                n => format!("Exported {name} \u{b7} {n} things left out or changed"),
            }),
            Err(e) => self.session.info(format!("Could not write {name}: {e}")),
        }
    }

    /// *Unlink*: the document stops watching its tokens file.
    pub(crate) fn unlink_tokens(&mut self) {
        let Some(link) = self.session.doc.tokens_link() else {
            return;
        };
        let (name, _) = where_label(Path::new(&link.path));
        self.session.doc.set_tokens_link(None);
        self.session.mark_unsaved();
        self.session.info(format!("Unlinked {name}"));
    }

    /// The linked file's review of what changed since it was last applied.
    pub(crate) fn open_linked_tokens(&mut self) {
        if let Some(link) = self.session.doc.tokens_link() {
            let path = PathBuf::from(&link.path);
            self.open_tokens_import(path, true);
        }
    }

    /// Read `path` and open its review; a file that is not DTCG is refused on
    /// the status line with the first reason and where.
    pub(crate) fn open_tokens_import(&mut self, path: PathBuf, from_link: bool) {
        let (name, _) = where_label(&path);
        match std::fs::read_to_string(&path) {
            Ok(text) => self.open_tokens_text(path, text, true, from_link),
            Err(e) => self.session.info(format!("Could not read {name}: {e}")),
        }
    }

    /// Open the review of `text`, read from `path` — a real file when
    /// `linkable`, a dropped file's name when not.
    pub(crate) fn open_tokens_text(
        &mut self,
        path: PathBuf,
        text: String,
        linkable: bool,
        from_link: bool,
    ) {
        let (name, _) = where_label(&path);
        let file = match dtcg_in::parse(&text) {
            Ok(f) => f,
            Err(r) => {
                self.session
                    .info(format!("{name} is not design tokens: {r}"));
                return;
            }
        };
        let into = self.shown_collection();
        let mut review =
            match dtcg_in::review(&self.session.doc, &file, into, &mut self.session.ids) {
                Ok(r) => r,
                Err(r) => {
                    self.session.info(format!("{name} cannot be read: {r}"));
                    return;
                }
            };
        let held = self.session.doc.tokens_link().cloned();
        let same = held
            .as_ref()
            .is_some_and(|l| Path::new(&l.path) == path.as_path());
        if from_link
            && let Some(l) = &held
            && let Ok(old) = dtcg_in::parse(&l.applied.to_string())
        {
            review.since(&file, &old);
        }
        self.palette = None;
        self.context_menu = None;
        self.open_menu = crate::app::TopMenu::None;
        let ticked = review.rows.iter().map(|r| r.ticked).collect();
        let mut t = TokensImport {
            path,
            text,
            review,
            ticked,
            filter: None,
            collapsed: HashSet::new(),
            link: linkable && (held.is_none() || same),
            linkable,
            from_link,
            why_not: None,
        };
        t.why_not = self.tokens_why_not(&t);
        self.tokens_import = Some(t);
    }

    fn tokens_why_not(&self, t: &TokensImport) -> Option<String> {
        if ticked_count(t) == 0 {
            return None;
        }
        t.review.ops(&self.session.doc, &t.ticked).err()
    }

    /// Apply the ticked rows as one undo step and record the link.
    fn commit_tokens_import(&mut self, t: &TokensImport) {
        let (name, _) = where_label(&t.path);
        let ops = match t.review.ops(&self.session.doc, &t.ticked) {
            Ok(ops) => ops,
            Err(why) => {
                self.session.info(why);
                return;
            }
        };
        let n = ticked_count(t);
        let mut said = None;
        if !ops.is_empty() {
            self.finish_text_first();
            self.session
                .name_step("Import tokens", icon::BRACKETS_CURLY);
            if !self.session.commit(Transaction(ops)) {
                self.session.info(format!("{name} could not be applied"));
                return;
            }
            said = Some(format!(
                "Imported {} from {name} \u{b7} Ctrl+Z to undo",
                changes_label(n)
            ));
        }
        let held = self.session.doc.tokens_link().cloned();
        let same = held
            .as_ref()
            .is_some_and(|l| Path::new(&l.path) == t.path.as_path());
        if t.link && t.linkable {
            if let Ok(applied) = serde_json::from_str::<serde_json::Value>(&t.text) {
                let link = TokensLink {
                    path: t.path.to_string_lossy().into_owned(),
                    applied,
                };
                if held.as_ref() != Some(&link) {
                    self.session.doc.set_tokens_link(Some(link));
                    self.session.mark_unsaved();
                }
                if said.is_none() {
                    said = Some(format!("Linked {name}"));
                }
            }
        } else if same {
            self.session.doc.set_tokens_link(None);
            self.session.mark_unsaved();
            said.get_or_insert_with(|| format!("Unlinked {name}"));
        }
        self.tokens_watch.key = None;
        if let Some(s) = said {
            self.session.info(s);
        }
    }

    /// Look at the linked file every [`WATCH_EVERY`] seconds and when the window
    /// regains the focus; a difference in its time or size recounts what it
    /// changed since it was last applied.
    pub(crate) fn watch_tokens(&mut self, ctx: &egui::Context) {
        let (now, focused) = ctx.input(|i| (i.time, i.viewport().focused.unwrap_or(true)));
        let regained = focused && !self.tokens_watch.focused;
        self.tokens_watch.focused = focused;
        let Some(link) = self.session.doc.tokens_link() else {
            self.tokens_watch.changes = 0;
            self.tokens_watch.key = None;
            return;
        };
        let moved = self
            .tokens_watch
            .key
            .as_ref()
            .is_none_or(|(p, _, _)| *p != link.path);
        if now < self.tokens_watch.next && !regained && !moved {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                self.tokens_watch.next - now,
            ));
            return;
        }
        self.tokens_watch.next = now + WATCH_EVERY;
        let stat = on_disk(Path::new(&link.path));
        let key = (link.path.clone(), stat, link.applied.to_string());
        if self.tokens_watch.key.as_ref() != Some(&key) {
            self.tokens_watch.changes = linked_changes(&self.session.doc, link);
            self.tokens_watch.key = Some(key);
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(WATCH_EVERY));
    }

    /// The pill beside the save state, before the kit pill: *tokens.json · 4
    /// changes*, or after *Later* its glyph and count. A click opens the review.
    pub(crate) fn tokens_pill(&mut self, ui: &mut egui::Ui) {
        let n = self.tokens_watch.changes;
        let Some(link) = self.session.doc.tokens_link() else {
            return;
        };
        if n == 0 {
            return;
        }
        let path = link.path.clone();
        let (name, _) = where_label(Path::new(&path));
        // The stat the watch last read, not a fresh one every frame.
        let seen = self.tokens_watch.key.as_ref().map(|(_, s, _)| *s);
        let quiet = self
            .tokens_watch
            .later
            .as_ref()
            .is_some_and(|(p, s)| *p == path && Some(*s) == seen);
        let text = match quiet {
            true => n.to_string(),
            false => format!("{name} \u{b7} {}", changes_label(n)),
        };
        let galley = ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap(
                text,
                egui::FontId::proportional(ui::SMALL_PT),
                color::accent_300(),
            )
        });
        let w = 9.0 + 13.0 + 6.0 + galley.size().x + 9.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::click());
        let p = ui.painter();
        let ground = match (resp.hovered(), quiet) {
            (true, _) => color::accent_a(56),
            (false, true) => color::accent_a(20),
            (false, false) => color::accent_a(40),
        };
        p.rect_filled(rect, egui::CornerRadius::same(6), ground);
        if !quiet {
            p.rect_stroke(
                rect,
                egui::CornerRadius::same(6),
                egui::Stroke::new(1.0, color::accent_a(100)),
                egui::StrokeKind::Inside,
            );
        }
        let y = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 9.0, y),
            egui::Align2::LEFT_CENTER,
            icon::BRACKETS_CURLY,
            theme::icon_font(13.0),
            color::accent_300(),
        );
        p.galley(
            egui::pos2(rect.left() + 28.0, y - galley.size().y / 2.0),
            galley,
            color::accent_300(),
        );
        let resp = resp.on_hover_text(format!(
            "{name} changed since it was last applied \u{b7} click to review"
        ));
        if resp.clicked() {
            self.open_linked_tokens();
        }
    }

    /// Draw the open review and answer its keys and clicks. A modal with a clear
    /// backdrop, as batch rename's: a click outside or `Escape` cancels.
    pub(crate) fn tokens_import_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut t) = self.tokens_import.take() else {
            return;
        };
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let area = ui::canvas_popup_area(ctx, egui::Id::new("tokens-import"), canvas);
        let mut act = Act::None;
        let mut changed = false;
        // Batch rename's: the settings card without its backdrop or margin.
        let mut modal = crate::settings::card_modal(ctx, "tokens-import")
            .area(area)
            .backdrop_color(egui::Color32::TRANSPARENT);
        modal.frame = modal.frame.map(|f| f.inner_margin(egui::Margin::ZERO));
        let modal = modal.show(ctx, |ui| {
            ui.set_width(W);
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            if head(ui, &t.path) {
                act = Act::Cancel;
            }
            filters(ui, &t.review, &mut t.filter);
            ui::card_rule(ui);
            changed |= list(ui, &t.review, &mut t.ticked, t.filter, &mut t.collapsed);
            if let Some(note) = note(&t.review) {
                ui::card_rule(ui);
                note_row(ui, &note);
            }
            ui::card_rule(ui);
            if let Some(a) = foot(ui, &mut t) {
                act = a;
            }
            ui::focus_ring(ui);
        });
        if modal.backdrop_response.clicked() {
            return;
        }
        if changed {
            t.why_not = self.tokens_why_not(&t);
        }
        if enter && t.why_not.is_none() {
            act = Act::Apply;
        }
        match act {
            Act::Cancel => {}
            Act::Later => {
                let path = t.path.to_string_lossy().into_owned();
                let stat = on_disk(&t.path);
                self.tokens_watch.later = Some((path, stat));
            }
            Act::Apply if t.why_not.is_none() => {
                self.commit_tokens_import(&t);
                ctx.request_repaint();
            }
            _ => self.tokens_import = Some(t),
        }
    }
}

enum Act {
    None,
    Apply,
    Cancel,
    Later,
}

fn ticked_count(t: &TokensImport) -> usize {
    t.review
        .rows
        .iter()
        .zip(&t.ticked)
        .filter(|(r, on)| **on && r.change != Change::Kit)
        .count()
}

fn changes_label(n: usize) -> String {
    match n {
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    }
}

/// A change's tag: its sign, ground and ink.
fn tag(c: Change) -> (&'static str, egui::Color32, egui::Color32) {
    let rgb = egui::Color32::from_rgb;
    match c {
        Change::Added => ("+", rgb(0x4f, 0x8f, 0xe6), rgb(0x0b, 0x12, 0x20)),
        Change::Removed => ("\u{2212}", rgb(0xe8, 0x96, 0x4a), rgb(0x1d, 0x12, 0x06)),
        Change::Changed => ("~", rgb(0xc9, 0xb4, 0x58), rgb(0x17, 0x14, 0x0a)),
        Change::Renamed => ("\u{2192}", color::text_a(77), color::text()),
        Change::Kit => ("K", color::text_a(40), theme::text::muted()),
    }
}

/// The title row: *Import tokens* in the settings card's title type, where the
/// file is, and the card's close button; true when that is clicked.
fn head(ui: &mut egui::Ui, path: &Path) -> bool {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 40.0), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    let title = p.text(
        egui::pos2(rect.left() + PAD, y),
        egui::Align2::LEFT_CENTER,
        "Import tokens",
        egui::FontId::proportional(ui::TITLE_PT),
        theme::text::strong(),
    );
    let (name, dirs) = where_label(path);
    let place = match dirs.is_empty() {
        true => name,
        false => format!("{name} \u{b7} {dirs}"),
    };
    let max = rect.right() - 40.0 - (title.right() + 8.0);
    let mut job = egui::text::LayoutJob::simple_singleline(
        place,
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::dim(),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max.max(40.0));
    let g = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().galley(
        egui::pos2(title.right() + 8.0, y - g.size().y / 2.0),
        g,
        theme::text::dim(),
    );
    let close = egui::Rect::from_center_size(
        egui::pos2(rect.right() - PAD - ui::CLOSE_BUTTON_W / 2.0, y),
        egui::Vec2::splat(ui::CLOSE_BUTTON_W),
    );
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(close));
    ui::close_button(&mut child).clicked()
}

/// The buttons that filter the list and carry the counts, *All* first: each a
/// [`ui::button_face`], `On` while it is the filter.
fn filters(ui: &mut egui::Ui, r: &Review, filter: &mut Option<Change>) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(W, ui::CONTROL_H + 10.0), egui::Sense::hover());
    let count = |c: Change| r.rows.iter().filter(|x| x.change == c).count();
    let chips = [
        (None, "All".to_string(), r.rows.len()),
        (
            Some(Change::Changed),
            "Changed".to_string(),
            count(Change::Changed),
        ),
        (
            Some(Change::Added),
            "Added".to_string(),
            count(Change::Added),
        ),
        (
            Some(Change::Renamed),
            "Renamed".to_string(),
            count(Change::Renamed),
        ),
        (
            Some(Change::Removed),
            "Removed".to_string(),
            count(Change::Removed),
        ),
        (
            Some(Change::Kit),
            "Kit\u{2019}s".to_string(),
            count(Change::Kit),
        ),
    ];
    let mut x = rect.left() + PAD;
    let y = rect.top() + 2.0;
    let font = egui::FontId::proportional(ui::CONTROL_PT);
    for (which, label, n) in chips {
        if which.is_some() && n == 0 {
            continue;
        }
        let on = *filter == which;
        let text = format!("{label} {n}");
        // Placeholder ink, so the galley takes the face's at paint.
        let g = ui.fonts_mut(|f| f.layout_no_wrap(text, font.clone(), egui::Color32::PLACEHOLDER));
        let sign = which.map(tag);
        let sign_w = if sign.is_some() { 12.0 } else { 0.0 };
        let w = ui::FIELD_PAD_X + sign_w + g.size().x + ui::FIELD_PAD_X;
        let chip = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, ui::CONTROL_H));
        let mut slot = ui.new_child(
            egui::UiBuilder::new()
                .id(ui.id().with(("tokens-chip", label.as_str())))
                .max_rect(chip),
        );
        let (_, resp, ink) = ui::button_face(&mut slot, chip.size(), FieldButton::on_if(on));
        let p = ui.painter();
        if let Some((s, bg, _)) = sign {
            let dot = if which == Some(Change::Renamed) {
                color::text()
            } else {
                bg
            };
            p.text(
                egui::pos2(chip.left() + 9.0 + 4.0, chip.center().y),
                egui::Align2::CENTER_CENTER,
                s,
                egui::FontId::proportional(ui::SMALL_PT),
                dot,
            );
        }
        p.galley(
            egui::pos2(
                chip.left() + 9.0 + sign_w,
                chip.center().y - g.size().y / 2.0,
            ),
            g,
            ink,
        );
        if resp.clicked() {
            *filter = which;
        }
        x += w + 6.0;
    }
}

/// The value columns' width for `n` modes: as wide as [`COL_W`], narrower when
/// many share the row.
fn col_w(n: usize) -> f32 {
    let n = n.max(1) as f32;
    let room = W - 2.0 * GAP - BOX_W - 180.0 - GAP * (n + 1.0);
    (room / n).clamp(72.0, COL_W)
}

/// The header, then the rows under their group headers, a closed group's
/// hidden; true when a box changed.
fn list(
    ui: &mut egui::Ui,
    r: &Review,
    ticked: &mut [bool],
    filter: Option<Change>,
    collapsed: &mut HashSet<String>,
) -> bool {
    let cw = col_w(r.columns.len());
    let (head, _) = ui.allocate_exact_size(egui::vec2(W, 28.0), egui::Sense::hover());
    let p = ui.painter();
    let font = egui::FontId::proportional(ui::EYEBROW_PT);
    let y = head.center().y + 2.0;
    let name_x = head.left() + GAP + 8.0 + BOX_W + GAP;
    ui::paint_eyebrow(p, egui::pos2(name_x, y), egui::Align2::LEFT_CENTER, "Token");
    let cols_x = head.right() - GAP - 8.0 - (cw + GAP) * r.columns.len() as f32 + GAP;
    for (i, c) in r.columns.iter().enumerate() {
        let mut job = egui::text::LayoutJob::simple_singleline(
            c.to_uppercase(),
            font.clone(),
            theme::text::faint(),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(cw);
        let g = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(
            egui::pos2(cols_x + (cw + GAP) * i as f32, y - g.size().y / 2.0),
            g,
            theme::text::faint(),
        );
    }
    let shown: Vec<usize> = (0..r.rows.len())
        .filter(|i| filter.is_none_or(|f| r.rows[*i].change == f))
        .collect();
    let mut changed = false;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: GAP as i8,
            right: GAP as i8,
            top: 0,
            bottom: 6,
        })
        .show(ui, |ui| {
            if shown.is_empty() {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(W - 2.0 * GAP, ROW_H), egui::Sense::hover());
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(rect.with_min_x(rect.left() + 8.0))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        ui::empty_note(
                            ui,
                            "Nothing to change: the document already holds what the file says",
                        )
                    },
                );
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("tokens-import-list")
                .max_height(LIST_H)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let mut group: Option<&str> = None;
                    let mut toggle = None;
                    for (n, i) in shown.iter().enumerate() {
                        let row = &r.rows[*i];
                        if group != Some(row.group.as_str()) {
                            group = Some(row.group.as_str());
                            if !row.group.is_empty() {
                                let count = shown[n..]
                                    .iter()
                                    .take_while(|j| r.rows[**j].group == row.group)
                                    .count();
                                let open = !collapsed.contains(&row.group);
                                if group_head(ui, &row.group, count, open) {
                                    toggle = Some(row.group.clone());
                                }
                            }
                        }
                        if collapsed.contains(&row.group) {
                            continue;
                        }
                        if row_ui(ui, row, &mut ticked[*i], cw, cols_x) {
                            changed = true;
                        }
                    }
                    // Alt-click closes or opens every group, as in the variables panel.
                    if let Some(g) = toggle {
                        let all = || {
                            r.rows
                                .iter()
                                .map(|x| x.group.clone())
                                .filter(|g| !g.is_empty())
                        };
                        match (ui.input(|i| i.modifiers.alt), collapsed.contains(&g)) {
                            (true, true) => collapsed.clear(),
                            (true, false) => collapsed.extend(all()),
                            (false, true) => _ = collapsed.remove(&g),
                            (false, false) => _ = collapsed.insert(g),
                        }
                    }
                });
        });
    changed
}

/// A group's header: its caret, name and the count of rows on show; true when
/// clicked.
fn group_head(ui: &mut egui::Ui, name: &str, count: usize, open: bool) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(W - 2.0 * GAP, GROUP_H), egui::Sense::click());
    let p = ui.painter();
    ui::list_row_ground(p, rect, false, resp.hovered());
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + 8.0, y),
        egui::Align2::LEFT_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        theme::icon_font(11.0),
        theme::text::faint(),
    );
    let label = p.text(
        egui::pos2(rect.left() + 8.0 + 18.0, y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::dim(),
    );
    p.text(
        egui::pos2(label.right() + 6.0, y),
        egui::Align2::LEFT_CENTER,
        count.to_string(),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    resp.clicked()
}

/// One row: its box, tag, name and second line, and a value per mode; true when
/// its box was clicked.
fn row_ui(ui: &mut egui::Ui, row: &Row, on: &mut bool, cw: f32, cols_x: f32) -> bool {
    let h = if row.sub.is_some() { ROW_H2 } else { ROW_H };
    let kit = row.change == Change::Kit;
    let sense = if kit {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(W - 2.0 * GAP, h), sense);
    let fade = |c: egui::Color32| match (kit, *on) {
        (true, _) => c.gamma_multiply(0.5),
        (false, false) => c.gamma_multiply(ui::UNTICKED_FADE),
        (false, true) => c,
    };
    let p = ui.painter();
    ui::list_row_ground(p, rect, false, resp.hovered() && !kit);
    let y = rect.center().y;
    // The box; a kit's row has a dead one, which the shared box has no state for.
    let at = egui::pos2(rect.left() + 8.0 + ui::TICK_BOX / 2.0, y);
    if kit {
        p.rect_stroke(
            egui::Rect::from_center_size(at, egui::vec2(ui::TICK_BOX, ui::TICK_BOX)),
            3.0,
            egui::Stroke::new(1.0, color::text_a(40)),
            egui::StrokeKind::Inside,
        );
    } else {
        ui::paint_tick_box(p, at, *on, resp.hovered());
    }
    // The tag, the name and its second line.
    let x = rect.left() + 8.0 + BOX_W + GAP;
    let line_y = if row.sub.is_some() {
        rect.top() + 13.0
    } else {
        y
    };
    let (sign, bg, ink) = tag(row.change);
    let t = egui::Rect::from_center_size(egui::pos2(x + 8.0, line_y), egui::vec2(16.0, 16.0));
    p.rect_filled(t, 3.0, fade(bg));
    p.text(
        t.center(),
        egui::Align2::CENTER_CENTER,
        sign,
        egui::FontId::proportional(ui::SMALL_PT),
        fade(ink),
    );
    let name_w = cols_x - GAP - (x + 23.0) - 4.0;
    let mut job = egui::text::LayoutJob::simple_singleline(
        row.name.clone(),
        egui::FontId::proportional(ui::CONTROL_PT),
        fade(color::text()),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(name_w);
    let g = ui.fonts_mut(|f| f.layout_job(job));
    // The whole text of whatever cut-short piece the pointer is on.
    let mut tip = None;
    let at = egui::pos2(x + 23.0, line_y - g.size().y / 2.0);
    if g.elided && ui.rect_contains_pointer(egui::Rect::from_min_size(at, g.size())) {
        tip = Some(row.name.clone());
    }
    p.galley(at, g, color::text());
    if let Some(sub) = &row.sub {
        let mut job = egui::text::LayoutJob::simple_singleline(
            sub.clone(),
            egui::FontId::proportional(ui::CAPTION_PT),
            fade(theme::text::dim()),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(name_w);
        let g = ui.fonts_mut(|f| f.layout_job(job));
        let at = egui::pos2(x + 23.0, rect.top() + 21.0);
        if g.elided && ui.rect_contains_pointer(egui::Rect::from_min_size(at, g.size())) {
            tip = Some(sub.clone());
        }
        p.galley(at, g, color::text());
    }
    // A value per mode: the old one struck through beside the new.
    for (i, c) in row.cells.iter().enumerate() {
        let cx = cols_x + (cw + GAP) * i as f32;
        let mut at = cx;
        let right = cx + cw;
        if let Some(old) = &c.old {
            at = value(ui, old, at, line_y, right, true, fade, &mut tip);
            at += 5.0;
        }
        if let Some(new) = &c.new {
            value(ui, new, at, line_y, right, false, fade, &mut tip);
        }
    }
    let clicked = resp.clicked() && !kit;
    if clicked {
        *on = !*on;
    }
    if let Some(tip) = tip {
        resp.on_hover_text_at_pointer(tip);
    } else if kit {
        resp.on_hover_text(
            "A kit\u{2019}s variable or style changes through the kit\u{2019}s own update",
        );
    }
    clicked
}

/// A swatch and a value from `x`, cut at `right`; struck and dim when `old`.
/// Returns where it ended, and sets `tip` to the whole value when it is cut
/// short under the pointer.
#[allow(clippy::too_many_arguments)]
fn value(
    ui: &egui::Ui,
    s: &Shown,
    mut x: f32,
    y: f32,
    right: f32,
    old: bool,
    fade: impl Fn(egui::Color32) -> egui::Color32,
    tip: &mut Option<String>,
) -> f32 {
    let p = ui.painter();
    if let Some(c) = s.swatch {
        let rgba = c.to_rgba8();
        let sw = egui::Rect::from_min_size(egui::pos2(x, y - 5.5), egui::vec2(11.0, 11.0));
        p.rect_filled(
            sw,
            2.0,
            fade(egui::Color32::from_rgba_unmultiplied(
                rgba.r, rgba.g, rgba.b, rgba.a,
            )),
        );
        p.rect_stroke(
            sw,
            2.0,
            egui::Stroke::new(1.0, color::text_a(30)),
            egui::StrokeKind::Inside,
        );
        x += 16.0;
    }
    let ink = if old {
        theme::text::faint()
    } else {
        theme::text::strong()
    };
    let fmt = egui::TextFormat {
        strikethrough: if old {
            egui::Stroke::new(1.0, fade(theme::text::faint()))
        } else {
            egui::Stroke::NONE
        },
        ..egui::TextFormat::simple(egui::FontId::proportional(ui::SMALL_PT), fade(ink))
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&s.text, 0.0, fmt);
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(10.0));
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let w = g.size().x;
    let at = egui::pos2(x, y - g.size().y / 2.0);
    if g.elided && ui.rect_contains_pointer(egui::Rect::from_min_size(at, g.size())) {
        *tip = Some(s.text.clone());
    }
    p.galley(at, g, color::text());
    x + w
}

/// What the review cannot apply and what it leaves alone, in one sentence or
/// two: tokens left out, the document's modes the file lacks.
fn note(r: &Review) -> Option<String> {
    let mut out = Vec::new();
    if !r.left_out.is_empty() {
        let named: Vec<String> = r
            .left_out
            .iter()
            .take(3)
            .map(|l| format!("{} ({})", l.path, l.ty))
            .collect();
        let more = r.left_out.len().saturating_sub(3);
        let list = match (named.as_slice(), more) {
            ([one], 0) => one.clone(),
            (many, 0) => {
                let (last, rest) = many.split_last().expect("not empty");
                format!("{} and {last}", rest.join(", "))
            }
            (many, n) => format!("{} and {n} more", many.join(", ")),
        };
        let n = r.left_out.len();
        out.push(format!(
            "{n} token{} left out: {list} {} no variable or style here.",
            if n == 1 { "" } else { "s" },
            if n == 1 { "has" } else { "have" }
        ));
    }
    match r.kept_modes.as_slice() {
        [] => {}
        [one] => out.push(format!(
            "The file has no {one} mode; its values here are kept."
        )),
        many => out.push(format!(
            "The file has no {} modes; their values here are kept.",
            many.join(", ")
        )),
    }
    (!out.is_empty()).then(|| out.join(" "))
}

fn note_row(ui: &mut egui::Ui, note: &str) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: PAD as i8,
            right: PAD as i8,
            top: 9,
            bottom: 9,
        })
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

/// *Keep this file linked*, then *Cancel* (or *Later*) and *Apply N changes*.
fn foot(ui: &mut egui::Ui, t: &mut TokensImport) -> Option<Act> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 50.0), egui::Sense::hover());
    if t.linkable {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    egui::pos2(rect.left() + PAD, rect.top()),
                    egui::pos2(rect.center().x, rect.bottom()),
                ))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        ui::tick_box(&mut child, "Keep this file linked", "", &mut t.link);
    }
    let n = ticked_count(t);
    let go = match n {
        0 => "Done".to_string(),
        n => format!("Apply {}", changes_label(n)),
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 11.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    child.spacing_mut().item_spacing.x = 8.0;
    let state = match &t.why_not {
        None => FieldButton::Primary,
        Some(_) => FieldButton::Disabled,
    };
    let primary = crate::settings::text_button(&mut child, &go, state);
    let primary = match &t.why_not {
        Some(why) => primary.on_hover_text(why.as_str()),
        None => primary,
    };
    let back = if t.from_link { "Later" } else { "Cancel" };
    let cancel = crate::settings::text_button(&mut child, back, FieldButton::Off);
    if let Some(why) = &t.why_not {
        let mut job = egui::text::LayoutJob::simple_singleline(
            why.clone(),
            egui::FontId::proportional(ui::CAPTION_PT),
            color::warn(),
        );
        job.wrap =
            egui::text::TextWrapping::truncate_at_width(cancel.rect.left() - rect.center().x - 8.0);
        let g = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(
            egui::pos2(
                cancel.rect.left() - 8.0 - g.size().x,
                rect.center().y - g.size().y / 2.0,
            ),
            g,
            color::warn(),
        );
    }
    if primary.clicked() && t.why_not.is_none() {
        Some(Act::Apply)
    } else if cancel.clicked() {
        Some(if t.from_link { Act::Later } else { Act::Cancel })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r##"{
  "$extensions": { "io.github.fadion.ondin": { "modes": [{ "name": "Light" }, { "name": "Dark" }] } },
  "brand": {
    "500": { "$type": "color", "$value": "#6d8cd9",
      "$extensions": { "io.github.fadion.ondin": { "modes": { "Light": "#6d8cd9", "Dark": "#8ba5e0" } } } }
  },
  "space": { "$type": "dimension", "gutter": { "$value": { "value": 24, "unit": "px" } } }
}"##;

    fn app() -> (egui::Context, OndinApp) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let app = OndinApp::headless(&ctx);
        (ctx, app)
    }

    fn temp_file(tag: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-tokens-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tokens.json");
        std::fs::write(&path, text).unwrap();
        path
    }

    /// Apply the open review as its boxes stand.
    fn apply(app: &mut OndinApp) {
        let t = app.tokens_import.take().expect("the review is open");
        assert_eq!(t.why_not, None);
        app.commit_tokens_import(&t);
    }

    /// The review opens on a file's rows, its modes added; Apply is one undo
    /// step named *Import tokens*, and the file is linked.
    #[test]
    fn apply_is_one_undo_step_and_links_the_file() {
        let (_ctx, mut app) = app();
        let path = temp_file("apply", FILE);
        app.open_tokens_import(path.clone(), false);
        let t = app.tokens_import.as_ref().expect("opened");
        let names: Vec<&str> = t.review.rows.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"brand.500") && names.contains(&"space.gutter"),
            "{names:?}"
        );
        assert!(t.link, "linking is on for a document linked to nothing");
        apply(&mut app);
        let doc = &app.session.doc;
        assert_eq!(doc.variables().modes().count(), 2);
        assert!(doc.variables().by_name("brand/500").is_some());
        let link = doc.tokens_link().expect("linked");
        assert_eq!(Path::new(&link.path), path.as_path());
        assert!(app.session.undo());
        assert!(
            app.session.doc.variables().is_empty(),
            "one step takes it all back"
        );
    }

    /// A linked file edited elsewhere is counted on the next look, and its pill's
    /// review opens on it. That the review keeps only what the file changed is
    /// `dtcg_in`'s `since`, pinned in that crate's tests.
    #[test]
    fn a_linked_file_changed_on_disk_is_counted() {
        let (ctx, mut app) = app();
        let path = temp_file("watch", FILE);
        app.open_tokens_import(path.clone(), false);
        apply(&mut app);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.watch_tokens(ui.ctx()));
        assert_eq!(app.tokens_watch.changes, 0, "just applied");
        std::fs::write(&path, FILE.replace("\"value\": 24", "\"value\": 32")).unwrap();
        // The next look, which a stat difference would bring; forced here, since
        // the size did not change and the clock may not have.
        app.tokens_watch.key = None;
        app.tokens_watch.next = 0.0;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| app.watch_tokens(ui.ctx()));
        assert_eq!(app.tokens_watch.changes, 1);
        app.open_linked_tokens();
        let t = app.tokens_import.as_ref().expect("opened");
        assert!(t.from_link);
        let names: Vec<&str> = t.review.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["space.gutter"]);
    }

    /// The review draws frame after frame without closing itself, its rows under
    /// their group headers, and `Escape` closes it applying nothing.
    #[test]
    fn the_review_draws_until_escape() {
        let (ctx, mut app) = app();
        let path = temp_file("draw", FILE);
        app.open_tokens_import(path, false);
        let canvas = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1200.0, 760.0));
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| app.tokens_import_ui(ui.ctx(), canvas));
        };
        for _ in 0..4 {
            frame(&mut app, vec![]);
        }
        assert!(app.tokens_import.is_some(), "still open");
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![escape]);
        assert!(app.tokens_import.is_none());
        assert!(app.session.doc.variables().is_empty(), "nothing applied");
    }

    /// A click on a group's header closes that group and a second opens it;
    /// Alt-click closes every group.
    /// flip: `group_head` sensing hover only → fails at "the click closes it".
    #[test]
    fn a_group_header_click_closes_and_opens_it() {
        let (ctx, mut app) = app();
        let path = temp_file("collapse", FILE);
        app.open_tokens_import(path, false);
        let canvas = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(1200.0, 760.0));
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>, modifiers| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                events,
                modifiers,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| app.tokens_import_ui(ui.ctx(), canvas));
        };
        let none = egui::Modifiers::NONE;
        for _ in 0..3 {
            frame(&mut app, vec![], none);
        }
        let boxed = ctx
            .memory(|m| m.area_rect(egui::Id::new("tokens-import")))
            .expect("the box is laid out");
        // Under the title, the chips, a divider and the column header.
        let at = egui::pos2(
            boxed.left() + 120.0,
            boxed.top() + 40.0 + 34.0 + 1.0 + 28.0 + 12.0,
        );
        let groups: HashSet<String> = {
            let t = app.tokens_import.as_ref().unwrap();
            t.review
                .rows
                .iter()
                .map(|r| r.group.clone())
                .filter(|g| !g.is_empty())
                .collect()
        };
        assert!(groups.len() >= 2, "the fixture has groups: {groups:?}");
        let first = {
            let t = app.tokens_import.as_ref().unwrap();
            t.review
                .rows
                .iter()
                .map(|r| &r.group)
                .find(|g| !g.is_empty())
                .unwrap()
                .clone()
        };
        let click = |app: &mut OndinApp, modifiers: egui::Modifiers| {
            frame(app, vec![egui::Event::PointerMoved(at)], modifiers);
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers,
            };
            frame(app, vec![button(true)], modifiers);
            frame(app, vec![button(false)], modifiers);
            for _ in 0..40 {
                frame(app, vec![], none);
            }
        };
        let collapsed = |app: &OndinApp| app.tokens_import.as_ref().unwrap().collapsed.clone();
        click(&mut app, none);
        assert_eq!(
            collapsed(&app),
            HashSet::from([first.clone()]),
            "the click closes it"
        );
        click(&mut app, none);
        assert!(collapsed(&app).is_empty(), "a second click opens it");
        click(&mut app, egui::Modifiers::ALT);
        assert_eq!(collapsed(&app), groups, "Alt-click closes every group");
    }

    /// A `.json` dropped on the window opens the review; a browser's drop, which
    /// has no path, cannot be linked.
    #[test]
    fn a_dropped_json_opens_the_review() {
        let (ctx, mut app) = app();
        let input = egui::RawInput {
            dropped_files: vec![egui::DroppedFile {
                name: "tokens.json".to_string(),
                bytes: Some(FILE.as_bytes().to_vec().into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| app.take_dropped_images(ui.ctx()));
        let t = app.tokens_import.as_ref().expect("the review opened");
        assert!(!t.linkable && !t.link);
    }

    /// A file that is not tokens is refused on the status line, and nothing opens.
    #[test]
    fn a_file_that_is_not_tokens_is_refused() {
        let (_ctx, mut app) = app();
        let path = temp_file("refuse", "{ \"a\": 1 }");
        app.open_tokens_import(path, false);
        assert!(app.tokens_import.is_none());
        assert_eq!(
            app.session.status().text,
            "tokens.json is not design tokens: a: a group or token must be an object"
        );
    }
}
