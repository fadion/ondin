//! Publishing a kit (§5.3e, 2A–2D; §15 D1015).
//!
//! A kit's top bar carries *Publish N* — the components changed since the last
//! publish, the working document frozen against the published state
//! (`ondin_core::kit::changes`) — which opens the dialog; so do the document
//! menu's *Publish kit…* and `Ctrl+Alt+P`. The dialog lists what changed, added /
//! changed / removed, takes an optional note, and publishes: the mark's version
//! goes up, the date is stamped, the frozen copy replaces the last one, and the
//! file is saved. **Not undoable** (§15 D1011 (2)) — an outward act, as *Save* is.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::kit::Changes;
use ondin_core::{Document, NodeId};

/// How many components a first publish lists before *+ N more* (2C).
const FIRST_LISTED: usize = 6;

/// The publish dialog's state while it is open.
#[derive(Default)]
pub(crate) struct PublishDraft {
    note: String,
}

impl OndinApp {
    /// This kit's changes since its last publish, or `None` when the document is
    /// not a shared kit. **Cached on the session's revision and the mark's
    /// version**: freezing a kit is a copy of every main, and the top bar asks
    /// every frame.
    pub(crate) fn kit_changes(&mut self) -> Option<Changes> {
        let version = self.shared_kit()?.version;
        let rev = self.session.revision();
        if let Some((r, v, c)) = &self.kit_changes
            && *r == rev
            && *v == version
        {
            return Some(c.clone());
        }
        let c = ondin_core::kit::changes(&self.session.doc);
        self.kit_changes = Some((rev, version, c.clone()));
        Some(c)
    }

    /// *Publish N* in a kit's top bar (2A): neutral, with the count; hovering
    /// gives the breakdown, and at 0 it stays, disabled, saying why.
    pub(crate) fn publish_button(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.kit_changes() else {
            return;
        };
        let version = self.shared_kit().map_or(0, |k| k.version);
        let n = c.count();
        let tip = if version == 0 {
            "Never published: the count is every component".to_string()
        } else if n == 0 && c.renamed {
            format!("The kit's name changed since v{version}")
        } else if n == 0 {
            format!("Nothing to publish · no changes since v{version}")
        } else {
            format!(
                "{n} component{} changed since v{version} · {}",
                plural(n),
                breakdown(&c)
            )
        };
        let enabled = c.any();
        let ink = if enabled {
            color::TEXT
        } else {
            theme::text::DISABLED
        };
        let label = ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap("Publish".into(), egui::FontId::proportional(11.5), ink)
        });
        let count = ui
            .ctx()
            .fonts_mut(|f| f.layout_no_wrap(n.to_string(), egui::FontId::proportional(10.5), ink));
        let chip_w = (count.size().x + 10.0).max(18.0);
        // The count's chip only when it counts something: a rename alone is
        // publishable with no component changed, and *Publish 0* would say there
        // is nothing to send (`arch-scribe`'s reading of D1015's fix).
        let chip = enabled && n > 0;
        let w = 10.0
            + 13.0
            + 7.0
            + label.size().x
            + if chip { 7.0 + chip_w } else { 0.0 }
            + 4.0
            + if chip { 0.0 } else { 6.0 };
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(w, 26.0),
            if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        let p = ui.painter();
        let ground = if enabled && resp.hovered() {
            color::text_a(26)
        } else {
            color::text_a(18)
        };
        p.rect_filled(rect, egui::CornerRadius::same(6), ground);
        let y = rect.center().y;
        let mut x = rect.left() + 10.0;
        p.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            icon::UPLOAD_SIMPLE,
            theme::icon_font(13.0),
            if enabled {
                color::text_a(153)
            } else {
                theme::text::DISABLED
            },
        );
        x += 13.0 + 7.0;
        p.galley(egui::pos2(x, y - label.size().y / 2.0), label.clone(), ink);
        x += label.size().x + 7.0;
        if chip {
            let chip = egui::Rect::from_min_size(egui::pos2(x, y - 9.0), egui::vec2(chip_w, 18.0));
            p.rect_filled(chip, egui::CornerRadius::same(4), color::text_a(26));
            p.galley(
                chip.center() - count.size() / 2.0,
                count,
                color::text_a(204),
            );
        }
        let resp = resp.on_hover_text(tip);
        if resp.clicked() {
            self.open_publish();
        }
    }

    /// Open the publish dialog — *Publish N*, *Publish kit…*, `Ctrl+Alt+P`. With
    /// nothing to publish it says so instead (2D).
    pub(crate) fn open_publish(&mut self) {
        let Some(c) = self.kit_changes() else {
            self.session
                .info("Make this document a kit first, from the menu on its name");
            return;
        };
        if !c.any() {
            let version = self.shared_kit().map_or(0, |k| k.version);
            self.session
                .info(format!("Nothing to publish · no changes since v{version}"));
            return;
        }
        self.publishing = Some(PublishDraft::default());
    }

    /// The publish dialog (2B, 2C), while it is open.
    pub(crate) fn publish_dialog(&mut self, ctx: &egui::Context) {
        if self.publishing.is_none() {
            return;
        }
        let Some(c) = self.kit_changes() else {
            self.publishing = None;
            return;
        };
        let mark = self.shared_kit().cloned().unwrap_or_default();
        let name = self.session.document_title();
        let first = mark.version == 0;
        let doc = &self.session.doc;
        let before = doc.published().map(|p| p.document().clone());
        let draft = self.publishing.as_mut().expect("checked above");
        let mut decision: Option<bool> = None;
        let modal = crate::settings::card("publish-kit", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(380.0, crate::settings::PAD));
            let title = if first {
                format!("Publish {name} for the first time")
            } else {
                format!("Publish {name} · v{}", mark.version + 1)
            };
            if crate::settings::modal_title(ui, &title) {
                decision = Some(false);
            }
            ui.add_space(4.0);
            let sub = if first {
                "Documents in your library can use these once you publish.".to_string()
            } else {
                let when = mark
                    .date
                    .map(|d| format!(" ({})", crate::library::clock::date_label(d)))
                    .unwrap_or_default();
                match (c.count(), c.renamed) {
                    (0, _) => format!("The kit's name changed since v{}{when}.", mark.version),
                    (n, renamed) => format!(
                        "{n} component{} changed since v{}{when}{}.",
                        plural(n),
                        mark.version,
                        if renamed { ", and the kit's name" } else { "" }
                    ),
                }
            };
            ui.label(
                egui::RichText::new(sub)
                    .size(12.0)
                    .color(theme::text::MUTED),
            );
            ui.add_space(10.0);
            if first {
                for id in c.added.iter().take(FIRST_LISTED) {
                    item_row(ui, doc, *id, None);
                }
                if c.added.len() > FIRST_LISTED {
                    ui.label(
                        egui::RichText::new(format!("+ {} more", c.added.len() - FIRST_LISTED))
                            .size(11.5)
                            .color(theme::text::DIM),
                    );
                }
            } else {
                section(ui, "Added", &c.added, doc, None);
                section(ui, "Changed", &c.changed, doc, None);
                if let Some(b) = &before {
                    section(
                        ui,
                        "Removed",
                        &c.removed,
                        b,
                        Some("kept as copy where used"),
                    );
                }
            }
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new("Note · optional")
                    .size(11.0)
                    .color(theme::text::DIM),
            );
            ui.add(
                egui::TextEdit::singleline(&mut draft.note)
                    .desired_width(f32::INFINITY)
                    .hint_text("What changed, in a line. Shown with the update."),
            );
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let verb = if first {
                    format!("Publish {} component{}", c.count(), plural(c.count()))
                } else {
                    "Publish".to_string()
                };
                if crate::settings::text_button(ui, &verb, crate::ui::FieldButton::On).clicked() {
                    decision = Some(true);
                }
                if crate::settings::text_button(ui, "Cancel", crate::ui::FieldButton::Off).clicked()
                {
                    decision = Some(false);
                }
            });
        });
        // `Escape` and the backdrop are *Cancel*, as on every card (§15 D525).
        if modal.should_close() {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                let note = self.publishing.take().map(|d| d.note).unwrap_or_default();
                self.publish_now(note);
            }
            Some(false) => self.publishing = None,
            None => {}
        }
    }

    /// Publish: freeze, bump, stamp, save — and say so (2D).
    pub(crate) fn publish_now(&mut self, note: String) {
        let count = self.kit_changes().map_or(0, |c| c.count());
        let Some(version) = ondin_core::kit::publish(
            &mut self.session.doc,
            Some(note),
            crate::library::clock::now(),
        ) else {
            self.session.fail("Nothing here to publish");
            return;
        };
        self.session.mark_unsaved();
        if !self.save_file(false) {
            return;
        }
        let name = self.session.document_title();
        self.session.info(match count {
            0 => format!("Published {name} v{version} · its new name"),
            n => format!("Published {name} v{version} · {n} component{}", plural(n)),
        });
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// *1 added, 2 changed, 1 removed* — the parts that are not zero.
fn breakdown(c: &Changes) -> String {
    [
        (c.added.len(), "added"),
        (c.changed.len(), "changed"),
        (c.removed.len(), "removed"),
    ]
    .iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, w)| format!("{n} {w}"))
    .collect::<Vec<_>>()
    .join(", ")
}

/// One of the dialog's groups — *Added 1* and its rows — when it has any.
fn section(ui: &mut egui::Ui, title: &str, ids: &[NodeId], doc: &Document, aside: Option<&str>) {
    if ids.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(11.0)
                .color(theme::text::DIM),
        );
        ui.label(
            egui::RichText::new(ids.len().to_string())
                .size(11.0)
                .color(theme::text::FAINT),
        );
    });
    for id in ids {
        item_row(ui, doc, *id, aside);
    }
    ui.add_space(6.0);
}

/// A component in the dialog: its glyph — a set's or a main's — its name, and on
/// the right a set's variant count or the row's aside.
fn item_row(ui: &mut egui::Ui, doc: &Document, id: NodeId, aside: Option<&str>) {
    let Some(n) = doc.get(id) else { return };
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    if n.set().is_some() {
        p.text(
            egui::pos2(rect.left() + 8.0, y),
            egui::Align2::CENTER_CENTER,
            icon::SQUARES_FOUR,
            theme::icon_font(12.0),
            theme::text::MUTED,
        );
    } else {
        crate::ui::paint_hexagon_filled(
            p,
            egui::pos2(rect.left() + 8.0, y),
            11.0,
            theme::text::MUTED,
        );
    }
    p.text(
        egui::pos2(rect.left() + 22.0, y),
        egui::Align2::LEFT_CENTER,
        n.name(),
        egui::FontId::proportional(12.0),
        color::TEXT,
    );
    let right = match (aside, n.set().is_some()) {
        (Some(a), _) => Some(a.to_string()),
        (None, true) => {
            // Its variants, not its children — a set may hold a plain layer too
            // (`arch-scribe`'s reading of the first cut).
            let k = ondin_core::variant::variants(doc, id).len();
            Some(format!("{k} variant{}", plural(k)))
        }
        (None, false) => None,
    };
    if let Some(r) = right {
        p.text(
            egui::pos2(rect.right() - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            r,
            egui::FontId::proportional(10.5),
            theme::text::FAINT,
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::app::OndinApp;
    use crate::library::state::Library;
    use eframe::egui;
    use ondin_core::kurbo::Size;
    use ondin_core::{NodeKind, Operation, Transaction};

    fn painted(ctx: &egui::Context, app: &mut OndinApp) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..2 {
            out = ctx
                .run_ui(Default::default(), |_| app.publish_dialog(ctx))
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect();
        }
        out
    }

    /// **A kit publishes from its dialog, and what it published is in the file**
    /// (§5.3e, 2A–2D; §15 D1015). Never published, the count is every component
    /// and the dialog's verb names it; publishing bumps the mark to v1, stamps it,
    /// keeps the note, saves — so the library scan reads v1 from the file's first
    /// 4 KB — and leaves nothing to publish, which the next ask says instead of
    /// opening the dialog. An edit after it is counted again.
    #[test]
    fn a_kit_publishes_and_the_file_says_so() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = std::env::temp_dir().join(format!("ondin-publish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = Library::open(root.clone());
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Atlas".into());
        app.session.doc.set_meta(meta);
        let main = app.session.ids.mint();
        let doc_root = app.session.doc.root();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: main,
                parent: doc_root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(80.0, 30.0),
                },
                transform: None,
                name: Some("Button".into()),
            },
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ])));
        app.make_kit();
        assert_eq!(
            app.kit_changes().unwrap().added,
            vec![main],
            "never published"
        );

        app.open_publish();
        assert!(app.publishing.is_some(), "the dialog opens");
        let text = painted(&ctx, &mut app);
        for want in [
            "Publish Atlas for the first time",
            "Button",
            "Publish 1 component",
        ] {
            assert!(text.contains(&want.to_string()), "{want}: {text:?}");
        }

        app.publishing = None;
        app.publish_now("First cut".into());
        let mark = app.session.doc.meta().kit.clone().unwrap();
        assert_eq!((mark.version, mark.note.as_deref()), (1, Some("First cut")));
        assert!(mark.date.is_some());
        assert!(
            app.session
                .status()
                .text
                .contains("Published Atlas v1 · 1 component")
        );
        let scanned = crate::library::scan::scan(&root);
        assert_eq!(
            scanned[0].meta.kit.as_ref().map(|k| k.version),
            Some(1),
            "in the file"
        );

        assert_eq!(app.kit_changes().unwrap().count(), 0);
        app.open_publish();
        assert!(app.publishing.is_none(), "nothing to publish");
        assert!(app.session.status().text.contains("no changes since v1"));

        assert!(app.session.commit(Transaction(vec![Operation::SetOpacity {
            id: main,
            opacity: 0.5,
        }])));
        assert_eq!(
            app.kit_changes().unwrap().changed,
            vec![main],
            "counted again"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A set's row counts its variants, not its children** (§15 D1015's fix):
    /// a set may hold a plain layer beside its variants, and the dialog read
    /// three for two.
    ///
    /// Flip, run: counting `children()` fails, the row reading *"3 variants"*.
    #[test]
    fn a_sets_row_counts_its_variants() {
        use ondin_core::variant::{VariantProp, VariantSet};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut meta = app.session.doc.meta().clone();
        meta.name = Some("Atlas".into());
        meta.kit = Some(Default::default());
        app.session.doc.set_meta(meta);
        let [set, small, large, note] = [(); 4].map(|_| app.session.ids.mint());
        let root = app.session.doc.root();
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 20.0),
            },
            transform: None,
            name: Some(name.into()),
        };
        assert!(app.session.commit(Transaction(vec![
            frame(set, root, 0, "Button"),
            frame(small, set, 0, "Small"),
            frame(large, set, 1, "Large"),
            frame(note, set, 2, "Note"),
            Operation::SetComponent {
                id: small,
                component: true,
            },
            Operation::SetComponent {
                id: large,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Small".into(), "Large".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: small,
                values: vec!["Small".into()],
            },
            Operation::SetVariant {
                id: large,
                values: vec!["Large".into()],
            },
        ])));
        assert_eq!(
            app.session.doc.get(set).unwrap().children().len(),
            3,
            "the fixture"
        );
        app.open_publish();
        let text = painted(&ctx, &mut app);
        assert!(text.contains(&"2 variants".to_string()), "{text:?}");
    }
}
