//! *Version history…* — the door to the copies `Ctrl+S` and *Save named version…*
//! pin under `.versions/<document id>/` (`library::store::pin_version`): a card
//! listing them newest first, each restorable and each nameable.
//!
//! **A restore loses nothing.** The current state is written and pinned first, so the
//! version it replaces is the newest entry in the same list; the restored document
//! keeps the file's path and id, and is saved at once. Undo does not cross it (the
//! history is the open document's), which is why the pin is unconditional.

use crate::app::OndinApp;
use crate::library::{clock, store};
use crate::theme::{self, color, icon};
use eframe::egui;
use std::path::{Path, PathBuf};

/// The card's width.
const CARD_W: f32 = 504.0;
/// How tall the list grows before it scrolls.
const LIST_MAX_H: f32 = 360.0;

/// One pinned copy as the card lists it.
pub(crate) struct Version {
    pub path: PathBuf,
    /// When it was pinned, read from its file name (`<unix seconds>[-n].ondin`).
    pub at: Option<u64>,
    /// The name the user gave it (`store::version_label`).
    pub label: Option<String>,
}

/// The name field open over one version's row.
pub(crate) struct Naming {
    pub path: PathBuf,
    pub text: String,
    /// Focus is requested once, on the field's first frame.
    pub focused: bool,
}

/// When a pinned copy was pinned, from its name (`store::version_stamp`).
pub(crate) fn stamp_of(path: &Path) -> Option<u64> {
    store::version_stamp(path).map(|(secs, _)| secs)
}

/// The name field over a version's row, filling what the row's buttons leave.
/// Answers `true` once the field lets go of focus, by Enter, Escape or a press
/// elsewhere; the caller tells Escape apart.
fn name_field(ui: &mut egui::Ui, n: &mut Naming) -> bool {
    let id = egui::Id::new(("version-name", &n.path));
    let size = egui::vec2(ui.available_width(), crate::settings::FIELD_H);
    let font = egui::FontId::proportional(12.5);
    let pad = crate::ui::edit_frame(ui, &font, size.y - crate::ui::FIELD_BORDER_H, 0.0);
    let resp = crate::ui::field_row(ui, size, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut n.text)
                .id(id)
                .hint_text(egui::RichText::new("Name this version").size(12.5))
                .char_limit(store::LABEL_MAX)
                // The row is the frame.
                .frame(pad)
                .desired_width(f32::INFINITY)
                .font(font),
        )
    });
    // Once, through the latch: requesting focus every frame would re-take the caret
    // on the frame that surrendered it, and `lost_focus` would never be true.
    if !n.focused {
        n.focused = true;
        resp.request_focus();
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(n.text.chars().count()),
        );
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), id);
        }
    }
    resp.lost_focus()
}

/// `"24 Oct 2026, 14:32"` in local time.
pub(crate) fn when_label(secs: u64) -> String {
    let local = (secs as i64 + clock::local_offset(secs)).max(0) as u64;
    let (h, m) = ((local % 86_400) / 3_600, (local % 3_600) / 60);
    format!("{}, {h:02}:{m:02}", clock::date_label(secs))
}

impl OndinApp {
    /// Whether the open document is a file the library lists, the one place
    /// `Ctrl+S` pins versions. A file elsewhere may carry a minted id, a library
    /// document's copy even, and is still not.
    pub(crate) fn in_library(&self) -> bool {
        self.session
            .path
            .as_deref()
            .is_some_and(|p| self.library.entry_at(p).is_some())
    }

    /// The open document's pinned versions, newest first; empty when it is not in
    /// the library or has no id to keep them under.
    pub(crate) fn document_versions(&self) -> Vec<Version> {
        if !self.in_library() {
            return Vec::new();
        }
        let Some(id) = self
            .session
            .doc
            .meta()
            .id
            .as_deref()
            .filter(|id| crate::library::ids::is_minted(id))
        else {
            return Vec::new();
        };
        store::versions(&self.library.root, id)
            .into_iter()
            .map(|path| Version {
                at: stamp_of(&path),
                label: store::version_label(&path),
                path,
            })
            .collect()
    }

    /// *Save named version…*: save, pin a version whatever the version-history
    /// preference says, since the ask is explicit, and open the card on that
    /// version's name field.
    pub(crate) fn save_named_version(&mut self) {
        self.finish_text_first();
        if !self.save_file(false) {
            return;
        }
        if !self.in_library() {
            return self
                .session
                .fail("Save this document to the library to keep its versions");
        }
        if let Some(path) = self.pin_saved_version() {
            self.versions_open = true;
            self.version_naming = Some(Naming {
                path,
                text: String::new(),
                focused: false,
            });
        }
    }

    /// Keep the name typed for `naming`'s version, or report why it was not kept.
    pub(crate) fn commit_version_name(&mut self, naming: Naming) {
        if let Err(e) = store::set_version_label(&naming.path, &naming.text) {
            let e = crate::private::io_error(&e);
            self.session.fail(format!("Naming the version failed: {e}"));
        }
    }

    /// The *Version history* card, while it is open.
    pub(crate) fn version_history_dialog(&mut self, ctx: &egui::Context) {
        if !self.versions_open {
            return;
        }
        let versions = self.document_versions();
        let filed = self.in_library();
        let title = format!("Version history of {}", self.shown_title());
        let mut close = false;
        let mut restore: Option<PathBuf> = None;
        let mut compare: Option<(PathBuf, String, Option<String>)> = None;
        let lit = self.versions_compare;
        let compared = self.session.compare.as_ref().map(|c| c.version.clone());
        let private = crate::private::on();
        let mut naming = self.version_naming.take();
        // Private mode draws no field, so a name cannot be typed blind; one open as
        // it came on keeps what was typed.
        if private && let Some(n) = naming.take() {
            self.commit_version_name(n);
        }
        // A name field open at the frame's start takes Escape from the card.
        let was_naming = naming.is_some();
        let mut name: Option<Naming> = None;
        let mut kept: Option<Naming> = None;
        let modal = crate::settings::card("version-history", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(CARD_W, crate::settings::PAD));
            if crate::settings::modal_title(ui, &title) {
                close = true;
            }
            ui.add_space(8.0);
            let empty = match (filed, versions.is_empty()) {
                (false, _) => Some("Save this document to the library to keep its versions."),
                (true, true) => Some("No versions yet — Ctrl+S saves one."),
                _ => None,
            };
            if let Some(words) = empty {
                ui.label(
                    egui::RichText::new(words)
                        .size(12.0)
                        .color(theme::text::dim()),
                );
            }
            egui::ScrollArea::vertical()
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    // A fixed row height, so the name field standing in for the two
                    // lines moves nothing.
                    let line =
                        |pt: f32| ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(pt)));
                    let row_h = (line(12.5) + 1.0 + line(11.0)).max(crate::settings::FIELD_H);
                    for (place, v) in versions.iter().enumerate() {
                        let editing = naming.as_ref().filter(|n| n.path == v.path).is_some();
                        let row = egui::vec2(ui.available_width(), row_h);
                        let layout = egui::Layout::left_to_right(egui::Align::Center);
                        // The version being compared stays marked.
                        if compared.as_ref() == Some(&v.path) {
                            let at = egui::Rect::from_min_size(ui.cursor().min, row)
                                .expand2(egui::vec2(4.0, 3.0));
                            ui.painter().rect_filled(at, 5.0, color::text_a(20));
                        }
                        ui.allocate_ui_with_layout(row, layout, |ui| {
                            ui.set_min_height(row_h);
                            ui.label(theme::icon_text(
                                icon::CLOCK_COUNTER_CLOCKWISE,
                                14.0,
                                theme::text::muted(),
                            ));
                            let (when, ago) = match v.at {
                                Some(at) => {
                                    (when_label(at), clock::relative_label(clock::since(at)))
                                }
                                None => ("Unknown time".into(), String::new()),
                            };
                            // A named version leads with its name, its time beneath.
                            // What the compare bar names it by: its name, and when beside it.
                            let named = match &v.label {
                                Some(label) => (
                                    crate::private::version(label, place).into_owned(),
                                    Some(when.clone()),
                                ),
                                None => (when.clone(), None),
                            };
                            let (head, foot) = match &v.label {
                                Some(label) => (
                                    crate::private::version(label, place).into_owned(),
                                    match ago.is_empty() {
                                        true => when,
                                        false => format!("{when} · {ago}"),
                                    },
                                ),
                                None => (when, ago),
                            };
                            if !editing {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 1.0;
                                    ui.label(
                                        egui::RichText::new(head).size(12.5).color(color::text()),
                                    );
                                    ui.label(
                                        egui::RichText::new(foot)
                                            .size(11.0)
                                            .color(theme::text::dim()),
                                    );
                                });
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if crate::settings::text_button(
                                        ui,
                                        "Restore",
                                        crate::ui::FieldButton::Off,
                                    )
                                    .on_hover_text(
                                        "Make this the document — the current state is kept \
                                         as the newest version",
                                    )
                                    .clicked()
                                    {
                                        restore = Some(v.path.clone());
                                    }
                                    let tip = match (private, &v.label) {
                                        (true, _) => crate::private::DECLINE_RENAME,
                                        (false, Some(_)) => "Rename this version",
                                        (false, None) => "Name this version",
                                    };
                                    let pencil = crate::ui::icon_button(
                                        ui,
                                        icon::PENCIL_SIMPLE,
                                        22.0,
                                        14.0,
                                        editing,
                                        !private,
                                    )
                                    .on_hover_text(tip);
                                    if pencil.clicked() && !private && !editing {
                                        name = Some(Naming {
                                            path: v.path.clone(),
                                            text: v.label.clone().unwrap_or_default(),
                                            focused: false,
                                        });
                                    }
                                    let diff = crate::ui::icon_button(
                                        ui,
                                        icon::GIT_DIFF,
                                        22.0,
                                        14.0,
                                        lit,
                                        true,
                                    )
                                    .on_hover_text("Compare with now");
                                    if diff.clicked() {
                                        compare = Some((v.path.clone(), named.0, named.1));
                                    }
                                    if let Some(n) = naming.as_mut().filter(|_| editing)
                                        && name_field(ui, n)
                                    {
                                        kept = naming.take();
                                    }
                                },
                            );
                        });
                        ui.add_space(6.0);
                    }
                });
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if crate::settings::text_button(ui, "Close", crate::ui::FieldButton::Off).clicked()
                {
                    close = true;
                }
            });
        });
        // Escape in the field is the field's: it surrendered focus this frame, so
        // `name_field` answered and `naming` is gone, and the card stays.
        let escaped = was_naming && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(n) = kept.filter(|_| !escaped) {
            self.commit_version_name(n);
        }
        // A press on another row's pencil while a field is open keeps the first name.
        if let Some(n) = name {
            if let Some(open) = naming.take() {
                self.commit_version_name(open);
            }
            naming = Some(n);
        }
        self.version_naming = naming;
        let leaving =
            restore.is_some() || compare.is_some() || close || (modal.should_close() && !escaped);
        // Leaving the card keeps what was typed, as a press elsewhere does.
        if leaving && let Some(n) = self.version_naming.take() {
            self.commit_version_name(n);
        }
        if leaving {
            self.versions_open = false;
            self.versions_compare = false;
        }
        if let Some(path) = restore {
            self.restore_version(&path);
        } else if let Some((path, title, when)) = compare {
            self.compare_with(&path, title, when);
        }
    }

    /// Make the pinned copy at `version` the open document: write and pin what is
    /// open now, then adopt the copy at the same path and save it. Reports what it
    /// did, or why it did nothing.
    pub(crate) fn restore_version(&mut self, version: &Path) {
        self.finish_text_first();
        let loaded = std::fs::read(version)
            .map_err(|e| e.to_string())
            .and_then(|b| ondin_core::io::load(&b).map_err(|e| e.to_string()));
        let mut doc = match loaded {
            Ok(doc) => doc,
            Err(e) => return self.session.fail(format!("Restore failed: {e}")),
        };
        let Some(path) = self.session.path.clone() else {
            return self
                .session
                .fail("Restore failed: this document is not saved");
        };
        // The current state first, written and pinned whatever the version-history
        // preference says: after the adopt it has no other record.
        if !self.save_file(false) {
            return;
        }
        self.library.refresh();
        let pinned = self
            .library
            .entry_at(&path)
            .cloned()
            .map(|entry| store::pin_version(&self.library.root, &entry));
        if !matches!(pinned, Some(Ok(Some(_)))) {
            return self
                .session
                .fail("Restore failed: the current state could not be kept as a version");
        }
        // The file's identity, whatever the copy says.
        let mut meta = doc.meta().clone();
        meta.id = self.session.doc.meta().id.clone();
        doc.set_meta(meta);
        // A restore is not an open: the user keeps looking where they were.
        let (page, camera) = (self.session.page, self.session.camera);
        self.adopt_opened(&path, doc);
        if self.session.page == page {
            self.session.camera = camera;
            self.session.home_pending = false;
        }
        self.session.mark_unsaved();
        if !self.save_file(false) {
            return;
        }
        let when = stamp_of(version).map(when_label).unwrap_or_default();
        self.session.info(format!(
            "Restored the version from {when} — the state before it is the newest version"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restore puts the pinned copy's content in the file under the same path and
    /// id, keeps the view where it was, and keeps the state it replaced as the
    /// newest version, so restoring that one comes back.
    /// flip: `restore_version` without its pin of the current state → fails at the
    /// version count; without putting the camera back → fails at "and is not homed".
    #[test]
    fn a_restore_keeps_the_state_it_replaces_as_the_newest_version() {
        let ctx = egui::Context::default();
        let root = std::env::temp_dir().join(format!("ondin-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        let name = |app: &OndinApp| app.session.doc.meta().name.clone();
        let rename = |app: &mut OndinApp, to: &str| {
            let mut meta = app.session.doc.meta().clone();
            meta.name = Some(to.into());
            app.session.doc.set_meta(meta);
            app.session.mark_unsaved();
        };
        rename(&mut app, "First");
        assert!(app.save_file(true), "filed and pinned");
        let path = app.session.path.clone().unwrap();
        let id = app.session.doc.meta().id.clone().unwrap();
        rename(&mut app, "Second");
        // Unsaved work at the moment of the restore: it must survive too.
        let first = app.document_versions();
        assert_eq!(first.len(), 1, "the fixture: one pin");
        // Zoomed in on a detail: the restore leaves the view there.
        app.session.home_pending = false;
        app.session.camera.center = ondin_core::kurbo::Point::new(321.0, 123.0);
        app.session.camera.set_zoom(4.0);
        let camera = app.session.camera;

        app.restore_version(&first[0].path);
        assert_eq!(app.session.camera, camera, "the view stays");
        assert!(!app.session.home_pending, "and is not homed");
        assert_eq!(name(&app).as_deref(), Some("First"), "the copy's content");
        assert_eq!(
            app.session.path.as_deref(),
            Some(path.as_path()),
            "same file"
        );
        assert_eq!(
            app.session.doc.meta().id.as_deref(),
            Some(id.as_str()),
            "same id"
        );
        assert!(!app.session.is_dirty(), "saved at once");
        let on_disk = ondin_core::io::load(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk.meta().name.as_deref(), Some("First"));

        let now = app.document_versions();
        assert_eq!(now.len(), 2, "the replaced state was pinned");
        app.restore_version(&now[0].path);
        assert_eq!(name(&app).as_deref(), Some("Second"), "and comes back");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A copy of a library document opened from outside the library, its id and
    /// all, is not in the library: no versions are listed for it, since `Ctrl+S`
    /// there pins none.
    ///
    /// flip: `in_library` answering for any saved document with an id → fails at
    /// "not in the library".
    #[test]
    fn a_document_outside_the_library_has_no_versions() {
        let ctx = egui::Context::default();
        let base = std::env::temp_dir().join(format!("ondin-versions-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("library"), base.join("downloads"));
        std::fs::create_dir_all(&outside).unwrap();
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        assert!(app.save_file(true), "filed and pinned");
        assert!(app.in_library(), "fixture: the original is in the library");
        assert_eq!(app.document_versions().len(), 1, "fixture: one pin");

        let copy = outside.join("brief.ondin");
        std::fs::copy(app.session.path.as_ref().unwrap(), &copy).unwrap();
        let doc = ondin_core::io::load(&std::fs::read(&copy).unwrap()).unwrap();
        assert!(doc.meta().id.is_some(), "fixture: the copy keeps the id");
        app.adopt_opened(&copy, doc);
        assert!(!app.in_library(), "not in the library");
        assert!(app.document_versions().is_empty(), "so no versions");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An app over a fresh library at a temporary `root`, its document filed there
    /// and no version pinned yet.
    fn filed(ctx: &egui::Context, tag: &str) -> (OndinApp, PathBuf) {
        crate::theme::install(ctx);
        let root =
            std::env::temp_dir().join(format!("ondin-versions-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        assert!(app.save_file(false), "fixture: filed");
        assert!(app.document_versions().is_empty(), "fixture: no pin yet");
        (app, root)
    }

    /// One frame of the card alone, answering the strings it drew.
    fn card_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<String> {
        card_texts(ctx, app, events)
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    /// `card_frame` with where each string was drawn.
    fn card_texts(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let time = ctx.input(|i| i.time) + 0.1;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.version_history_dialog(ui.ctx()))
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos)),
                _ => None,
            })
            .collect()
    }

    /// Opening a row's name field moves no row: every *Restore* sits where it sat.
    ///
    /// flip: the row sized by its content rather than `row_h` → fails at "no row
    /// moves".
    #[test]
    fn opening_a_name_field_moves_no_row() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "steady");
        assert!(app.save_file(true), "fixture: one pin");
        assert!(app.save_file(true), "fixture: two pins");
        let versions = app.document_versions();
        assert_eq!(versions.len(), 2, "fixture: two rows");
        app.versions_open = true;
        let restores = |drawn: Vec<(String, egui::Pos2)>| -> Vec<f32> {
            drawn
                .into_iter()
                .filter(|(t, _)| t == "Restore")
                .map(|(_, p)| p.y)
                .collect()
        };
        for _ in 0..4 {
            card_texts(&ctx, &mut app, Vec::new());
        }
        let before = restores(card_texts(&ctx, &mut app, Vec::new()));
        app.version_naming = Some(Naming {
            path: versions[0].path.clone(),
            text: String::new(),
            focused: false,
        });
        for _ in 0..4 {
            card_texts(&ctx, &mut app, Vec::new());
        }
        let after = restores(card_texts(&ctx, &mut app, Vec::new()));
        assert!(app.version_naming.is_some(), "fixture: the field is open");
        assert_eq!(before.len(), 2, "fixture: both rows drawn");
        assert_eq!(before, after, "no row moves");
        let _ = std::fs::remove_dir_all(&root);
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

    /// *Save named version…* pins a version even with the version-history
    /// preference off, opens the card on that version's focused name field, and
    /// Enter keeps the name, which the row then leads with.
    ///
    /// flip: `version_history_dialog` dropping the name `name_field` let go of →
    /// fails at "the name is kept".
    #[test]
    fn a_named_version_is_pinned_and_named_through_its_field() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "named");
        app.prefs.version_history = false;
        app.save_named_version();
        let versions = app.document_versions();
        assert_eq!(versions.len(), 1, "pinned whatever the preference says");
        assert!(app.versions_open, "the card opens");
        let field = egui::Id::new(("version-name", &versions[0].path));
        card_frame(&ctx, &mut app, Vec::new());
        card_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(field),
            "on its name field"
        );

        card_frame(
            &ctx,
            &mut app,
            vec![egui::Event::Text("Sent to client".into())],
        );
        card_frame(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        let drawn = card_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            store::version_label(&versions[0].path).as_deref(),
            Some("Sent to client"),
            "the name is kept"
        );
        assert!(app.version_naming.is_none(), "the field closes");
        assert!(app.versions_open, "and the card stays");
        assert!(
            drawn.iter().any(|t| t == "Sent to client"),
            "the row leads with it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Escape in the name field drops what was typed and closes only the field; a
    /// second Escape closes the card.
    ///
    /// flip: `version_history_dialog` letting the card close on the field's Escape
    /// → fails at "the card stays".
    #[test]
    fn escape_in_the_name_field_closes_only_the_field() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "escape");
        app.save_named_version();
        let path = app.document_versions()[0].path.clone();
        card_frame(&ctx, &mut app, Vec::new());
        card_frame(&ctx, &mut app, Vec::new());
        card_frame(&ctx, &mut app, vec![egui::Event::Text("Draft".into())]);
        assert!(app.version_naming.is_some(), "fixture: the field is open");
        card_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        card_frame(&ctx, &mut app, Vec::new());
        assert_eq!(store::version_label(&path), None, "nothing typed is kept");
        assert!(app.version_naming.is_none(), "the field closes");
        assert!(app.versions_open, "the card stays");
        card_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(!app.versions_open, "a second Escape closes it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// In private mode a version's name is drawn as its place, and no name field
    /// opens, so a name cannot be typed blind; one open as it came on is kept.
    ///
    /// flip: the row drawing `v.label` without `private::version` → fails at "the
    /// name is masked".
    #[test]
    fn private_mode_masks_a_versions_name() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "private");
        assert!(app.save_file(true), "fixture: one pin");
        let path = app.document_versions()[0].path.clone();
        store::set_version_label(&path, "Acme pitch").unwrap();
        app.versions_open = true;
        crate::private::set(true);
        app.version_naming = Some(Naming {
            path: path.clone(),
            text: "Acme".into(),
            focused: false,
        });
        let mut drawn = Vec::new();
        for _ in 0..2 {
            drawn.extend(card_frame(&ctx, &mut app, Vec::new()));
        }
        crate::private::set(false);
        assert!(
            !drawn.iter().any(|t| t.contains("Acme")),
            "the name is masked: {drawn:?}"
        );
        assert!(drawn.iter().any(|t| t == "Version 1"), "drawn as its place");
        assert!(app.version_naming.is_none(), "no field opens");
        assert_eq!(
            store::version_label(&path).as_deref(),
            Some("Acme"),
            "the field open as it came on keeps what was typed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
