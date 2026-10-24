//! *Version history…* — the door to the copies `Ctrl+S` pins under
//! `.versions/<document id>/` (`library::store::pin_version`): a card listing them
//! newest first, each restorable.
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

/// The card's width, *Kits used…*'s.
const CARD_W: f32 = 420.0;
/// How tall the list grows before it scrolls.
const LIST_MAX_H: f32 = 360.0;

/// One pinned copy as the card lists it.
pub(crate) struct Version {
    pub path: PathBuf,
    /// When it was pinned, read from its file name (`<unix seconds>[-n].ondin`).
    pub at: Option<u64>,
}

/// When a pinned copy was pinned, from its name (`store::version_stamp`).
pub(crate) fn stamp_of(path: &Path) -> Option<u64> {
    store::version_stamp(path).map(|(secs, _)| secs)
}

/// `"24 Oct 2026, 14:32"` in local time.
pub(crate) fn when_label(secs: u64) -> String {
    let local = (secs as i64 + clock::local_offset(secs)).max(0) as u64;
    let (h, m) = ((local % 86_400) / 3_600, (local % 3_600) / 60);
    format!("{}, {h:02}:{m:02}", clock::date_label(secs))
}

impl OndinApp {
    /// The open document's pinned versions, newest first; empty when it has no
    /// library id to keep them under.
    pub(crate) fn document_versions(&self) -> Vec<Version> {
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
                path,
            })
            .collect()
    }

    /// The *Version history* card, while it is open.
    pub(crate) fn version_history_dialog(&mut self, ctx: &egui::Context) {
        if !self.versions_open {
            return;
        }
        let versions = self.document_versions();
        let filed = self.session.path.is_some() && self.session.doc.meta().id.is_some();
        let title = format!("Version history of {}", self.session.document_title());
        let mut close = false;
        let mut restore: Option<PathBuf> = None;
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
                        .color(theme::text::DIM),
                );
            }
            egui::ScrollArea::vertical()
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for v in &versions {
                        ui.horizontal(|ui| {
                            ui.label(theme::icon_text(
                                icon::CLOCK_COUNTER_CLOCKWISE,
                                14.0,
                                theme::text::MUTED,
                            ));
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                let (when, ago) = match v.at {
                                    Some(at) => {
                                        (when_label(at), clock::relative_label(clock::since(at)))
                                    }
                                    None => ("Unknown time".into(), String::new()),
                                };
                                ui.label(egui::RichText::new(when).size(12.5).color(color::TEXT));
                                ui.label(
                                    egui::RichText::new(ago).size(11.0).color(theme::text::DIM),
                                );
                            });
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
        if let Some(path) = restore {
            self.versions_open = false;
            self.restore_version(&path);
        } else if close || modal.should_close() {
            self.versions_open = false;
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
        self.adopt_opened(&path, doc);
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
    /// id, and keeps the state it replaced as the newest version, so restoring that
    /// one comes back.
    /// flip: `restore_version` without its pin of the current state → fails at the
    /// version count.
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

        app.restore_version(&first[0].path);
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
}
