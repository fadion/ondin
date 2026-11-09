//! Publishing a kit (§5.3e; §15 D1015).
//!
//! A kit's top bar carries *Publish N* — the components changed since the last
//! publish, the working document frozen against the published state
//! (`ondin_core::kit::changes`) — which opens the popover under it; so do the
//! document menu's *Publish kit…* and `Ctrl+Alt+P`. The popover lists what changed, added /
//! changed / removed, takes an optional note, and publishes: the mark's version
//! goes up, the date is stamped, the frozen copy replaces the last one, and the
//! file is saved. **Not undoable** (§15 D1011 (2)) — an outward act, as *Save* is.

use crate::app::{OndinApp, TopMenu};
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::kit::Changes;
use ondin_core::{Document, NodeId};

/// How many components a first publish lists before *+ N more*.
const FIRST_LISTED: usize = 6;
/// Where *Publish N* stood this frame, with the pass it was drawn in: the
/// popover hangs from it.
const HEAD_ID: &str = "publish-kit-head";
/// What the popover holds besides its list — title, line, note, buttons and
/// padding — taken from the room below the button before the list's cap.
const CHROME_H: f32 = 170.0;

/// The publish popover's state while it is open.
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

    /// *Publish N* in a kit's top bar, with the count; hovering gives the
    /// breakdown, and at 0 it stays, disabled, saying why.
    ///
    /// A [`FieldButton::Primary`] pill at the toolbar scale, as the library's *New
    /// file* is: publishing is the one thing a kit's bar is for. Disabled, it falls
    /// back to the neutral ground, so a kit with nothing to send does not wear the
    /// colour of an action.
    pub(crate) fn publish_button(&mut self, ui: &mut egui::Ui) {
        let Some(c) = self.kit_changes() else {
            return;
        };
        let version = self.shared_kit().map_or(0, |k| k.version);
        let n = c.count();
        let tip = if version == 0 {
            "Never published: the count is every component".to_string()
        } else if n == 0 && c.any() {
            format!("{} changed since v{version}", not_a_component(&c))
        } else if n == 0 {
            format!("Nothing to publish · no changes since v{version}")
        } else {
            format!(
                "{} changed since v{version} · {}",
                ui::count(n, "component"),
                breakdown(&c)
            )
        };
        let enabled = c.any();
        let scale = ui::Scale::Toolbar;
        let glyph_pt = scale.glyph_pt();
        let label_w = ui
            .ctx()
            .fonts_mut(|f| {
                f.layout_no_wrap(
                    "Publish".into(),
                    egui::FontId::proportional(scale.text_pt()),
                    color::text(),
                )
            })
            .size()
            .x;
        let count_w = ui
            .ctx()
            .fonts_mut(|f| {
                f.layout_no_wrap(
                    n.to_string(),
                    egui::FontId::proportional(ui::CAPTION_PT),
                    color::text(),
                )
            })
            .size()
            .x;
        let chip_w = (count_w + 10.0).max(16.0);
        // The count's chip only when it counts something: a rename alone is
        // publishable with no component changed, and *Publish 0* would say there
        // is nothing to send.
        let chip = enabled && n > 0;
        let w = 10.0
            + glyph_pt
            + 7.0
            + label_w
            + if chip { 7.0 + chip_w } else { 0.0 }
            + 4.0
            + if chip { 0.0 } else { 6.0 };
        let state = match enabled {
            true => FieldButton::Primary,
            false => FieldButton::Disabled,
        };
        let (rect, resp, fg) = ui::button_face(ui, egui::vec2(w, ui::TOOLBAR_H), state);
        let p = ui.painter();
        let y = rect.center().y;
        let mut x = rect.left() + 10.0;
        p.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            icon::UPLOAD_SIMPLE,
            theme::icon_font(glyph_pt),
            fg,
        );
        x += glyph_pt + 7.0;
        p.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            "Publish",
            egui::FontId::proportional(scale.text_pt()),
            fg,
        );
        x += label_w + 7.0;
        if chip {
            let chip = egui::Rect::from_min_size(egui::pos2(x, y - 8.0), egui::vec2(chip_w, 16.0));
            p.rect_filled(chip, egui::CornerRadius::same(4), color::accent_600());
            p.text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                n.to_string(),
                egui::FontId::proportional(ui::CAPTION_PT),
                color::accent_100(),
            );
        }
        let open = self.publishing.is_some();
        let resp = match open {
            true => resp,
            false => resp.on_hover_text(tip),
        };
        // Where the popover hangs, this frame (`publish_popover`).
        let pass = ui.ctx().cumulative_pass_nr();
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new(HEAD_ID), (pass, rect)));
        if resp.clicked() && enabled {
            match open {
                true => self.close_publish(),
                false => self.open_publish(),
            }
        }
    }

    /// Close the publish popover, its note let go.
    fn close_publish(&mut self) {
        self.publishing = None;
        if self.open_menu == TopMenu::Publish {
            self.open_menu = TopMenu::None;
        }
    }

    /// Open the publish popover — *Publish N*, *Publish kit…*, `Ctrl+Alt+P`. With
    /// nothing to publish it says so instead; open already, it keeps its note.
    pub(crate) fn open_publish(&mut self) {
        if self.refused_by_compare() {
            return;
        }
        // The one seam for the button, the menu row and the chord: a publish
        // under a live text session froze the main without what was typed and
        // spent the version on it, and the popover's change list has to be
        // counted from the committed text too.
        self.finish_text_first();
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
        self.publishing.get_or_insert_with(PublishDraft::default);
        self.open_menu = TopMenu::Publish;
    }

    /// The publish popover, while it is open: the kit update popover's card,
    /// hung under *Publish N* — or, where the top bar is not drawn (focus mode),
    /// at the top of the window's middle. What changed by section, scrolling past
    /// the room there is, an optional note, and *Cancel* / *Publish*. A click
    /// outside it or `Escape` lets it go, as any top-bar dropdown.
    pub(crate) fn publish_popover(&mut self, ctx: &egui::Context) {
        // Let go last frame, or the slot taken by another top-bar menu: the
        // note goes a frame after the `Escape` that closed it, so that press is
        // not also read as leaving focus mode (`modal_is_up`).
        if self.publishing.is_some() && self.open_menu != TopMenu::Publish {
            self.publishing = None;
        }
        if self.publishing.is_none() {
            if self.open_menu == TopMenu::Publish {
                self.open_menu = TopMenu::None;
            }
            return;
        }
        let Some(c) = self.kit_changes() else {
            self.close_publish();
            return;
        };
        let mark = self.shared_kit().cloned().unwrap_or_default();
        let name = self.shown_title();
        let first = mark.version == 0;
        let before = self.session.doc.published().map(|p| p.document().clone());
        let pass = ctx.cumulative_pass_nr();
        let head: Option<(u64, egui::Rect)> = ctx.data(|d| d.get_temp(egui::Id::new(HEAD_ID)));
        let anchor = match head {
            Some((p, r)) if p == pass => egui::pos2(r.left(), r.bottom() + ui::MENU_GAP),
            _ => {
                let screen = ctx.content_rect();
                egui::pos2(
                    screen.center().x - super::updates::POP_W / 2.0,
                    screen.top() + ui::CANVAS_POPUP_TOP,
                )
            }
        };
        let list_max = ui::popover_list_max(ctx, anchor.y, CHROME_H, super::updates::LIST_MAX_H);
        let mut decision: Option<bool> = None;
        let (w, pad) = (super::updates::POP_W, ui::POPOVER_PAD);
        self.dropdown_padded(ctx, "publish-kit", anchor, w, pad, None, |app, ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            let title = if first {
                format!("Publish {name} for the first time")
            } else {
                format!("Publish {name} · v{}", mark.version + 1)
            };
            ui.label(
                egui::RichText::new(title)
                    .size(ui::CONTROL_PT)
                    .color(theme::text::strong()),
            );
            let sub = if first {
                "Documents in your library can use these once you publish.".to_string()
            } else {
                let when = mark
                    .date
                    .map(|d| format!(" ({})", crate::library::clock::date_label(d)))
                    .unwrap_or_default();
                match c.count() {
                    0 => format!(
                        "{} changed since v{}{when}.",
                        not_a_component(&c),
                        mark.version
                    ),
                    n => format!(
                        "{} changed since v{}{when}{}.",
                        ui::count(n, "component"),
                        mark.version,
                        // And what else travels (§15 D1064's variables and
                        // styles beside the name and the order).
                        if c.renamed || c.reordered || c.tokens {
                            format!(
                                ", and {}",
                                joined(&[
                                    (c.renamed, "the kit's name"),
                                    (c.reordered, "their order"),
                                    (c.tokens, "the variables and styles they read"),
                                ])
                            )
                        } else {
                            String::new()
                        }
                    ),
                }
            };
            ui.label(
                egui::RichText::new(sub)
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
            );
            ui.add_space(4.0);
            let doc = &app.session.doc;
            ui::fit_scroll(ui, "publish-kit-list", list_max, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                if first {
                    for id in c.added.iter().take(FIRST_LISTED) {
                        item_row(ui, doc, *id, None);
                    }
                    if c.added.len() > FIRST_LISTED {
                        ui.label(
                            egui::RichText::new(format!("+ {} more", c.added.len() - FIRST_LISTED))
                                .size(ui::SMALL_PT)
                                .color(theme::text::dim()),
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
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Note · optional")
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
            );
            // The app's text field, not a bare `TextEdit`: egui's own frame has
            // almost no inset and a focus ring nothing else draws (`ui::text_field`).
            if let Some(draft) = app.publishing.as_mut() {
                ui::text_field(
                    ui,
                    egui::vec2(ui.available_width(), ui::CONTROL_H),
                    &mut draft.note,
                    "What changed, in a line. Shown with the update.",
                );
            }
            ui.add_space(6.0);
            let verb = if first {
                format!("Publish {}", ui::count(c.count(), "component"))
            } else {
                "Publish".to_string()
            };
            let buttons = [
                ("Cancel", FieldButton::Off),
                (verb.as_str(), FieldButton::Primary),
            ];
            match crate::settings::footer_row(ui, &buttons) {
                Some(0) => decision = Some(false),
                Some(_) => decision = Some(true),
                None => {}
            }
        });
        match decision {
            Some(true) => {
                let note = self.publishing.take().map(|d| d.note).unwrap_or_default();
                self.close_publish();
                self.publish_now(note);
            }
            Some(false) => self.close_publish(),
            None => {}
        }
    }

    /// Publish: freeze, bump, stamp, save — and say so.
    pub(crate) fn publish_now(&mut self, note: String) {
        // The writer refuses as well as the popover's door, for any other caller.
        if self.refused_by_compare() {
            return;
        }
        // `open_publish` finished it; this is the verb, and any other caller.
        self.finish_text_first();
        let changes = self.kit_changes();
        let count = changes.as_ref().map_or(0, |c| c.count());
        // What a publish of no component carried, for its toast: the zero
        // arm was written for the name alone, and said *"its new name"* over
        // a publish of the order (the record's reading of `0d540f8`).
        let other = changes.as_ref().map_or("its new name".to_string(), |c| {
            joined(&[
                (c.renamed, "its new name"),
                (c.reordered, "its components' new order"),
                (c.tokens, "its variables and styles"),
            ])
        });
        // Mains on draft pages a published main uses go too, and are named.
        let carried: Vec<String> = ondin_core::kit::carried_from_drafts(&self.session.doc)
            .into_iter()
            .filter_map(|id| {
                let doc = &self.session.doc;
                doc.get(id)
                    .map(|_| crate::private::layer(doc, id).into_owned())
            })
            .collect();
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
        let name = self.shown_title();
        let drafts = match carried.as_slice() {
            [] => String::new(),
            [one] => format!(" · {one} from a draft page, which a component uses"),
            many => format!(
                " · {} from draft pages, which components use",
                many.join(", ")
            ),
        };
        self.session.info(match count {
            0 => format!("Published {name} v{version} · {other}{drafts}"),
            n if changes.as_ref().is_some_and(|c| c.tokens) => format!(
                "Published {name} v{version} · {} and its variables and styles{drafts}",
                ui::count(n, "component")
            ),
            n => format!(
                "Published {name} v{version} · {}{drafts}",
                ui::count(n, "component")
            ),
        });
    }
}

/// What changed that is not a component — the kit's name, the order of its
/// components, the variables and styles they read (§15 D1064), or any of them
/// together — as the subject of *"… changed since v4"*.
fn not_a_component(c: &Changes) -> String {
    let mut s = joined(&[
        (c.renamed, "the kit's name"),
        (c.reordered, "the order of its components"),
        // A variable or a style its components read (§15 D1064) — not a rename.
        (c.tokens, "the variables and styles its components read"),
    ]);
    if let Some(first) = s.get(..1) {
        let up = first.to_uppercase();
        s.replace_range(..1, &up);
    }
    s
}

/// The parts that hold, as *a, b and c* — the kit's name when none does, the
/// zero arm's old answer.
fn joined(parts: &[(bool, &str)]) -> String {
    let on: Vec<&str> = parts.iter().filter(|(b, _)| *b).map(|(_, s)| *s).collect();
    match on.as_slice() {
        [] => parts.first().map_or(String::new(), |(_, s)| s.to_string()),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
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

/// One of the popover's groups — an eyebrow, *Changed · 2*, as the kit update
/// review's and *Bind other uses*' are, and its rows — when it has any.
fn section(ui: &mut egui::Ui, title: &str, ids: &[NodeId], doc: &Document, aside: Option<&str>) {
    if ids.is_empty() {
        return;
    }
    ui.add_space(4.0);
    ui.label(ui::eyebrow(&format!("{title} · {}", ids.len())));
    ui.add_space(2.0);
    for id in ids {
        item_row(ui, doc, *id, aside);
    }
}

/// A component in the popover: its glyph — a set's or a main's — its name, and
/// on the right a set's variant count or the row's aside. A name too long for
/// the row is cut short of the aside and names itself in a tooltip.
fn item_row(ui: &mut egui::Ui, doc: &Document, id: NodeId, aside: Option<&str>) {
    let Some(n) = doc.get(id) else { return };
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ui::MENU_ROW_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let y = rect.center().y;
    if n.set().is_some() {
        p.text(
            egui::pos2(rect.left() + 8.0, y),
            egui::Align2::CENTER_CENTER,
            icon::VARIANT_SET,
            theme::icon_font(12.0),
            theme::text::muted(),
        );
    } else {
        crate::ui::paint_hexagon_filled(
            p,
            egui::pos2(rect.left() + 8.0, y),
            11.0,
            theme::text::muted(),
        );
    }
    let right = match (aside, n.set().is_some()) {
        (Some(a), _) => Some(a.to_string()),
        (None, true) => {
            // Its variants, not its children — a set may hold a plain layer too.
            let k = ondin_core::variant::variants(doc, id).len();
            Some(ui::count(k, "variant"))
        }
        (None, false) => None,
    };
    let tail_left = match right {
        Some(r) => p
            .text(
                egui::pos2(rect.right() - 4.0, y),
                egui::Align2::RIGHT_CENTER,
                r,
                egui::FontId::proportional(ui::CAPTION_PT),
                theme::text::faint(),
            )
            .left(),
        None => rect.right(),
    };
    let name = crate::private::layer(doc, id);
    let mut job = egui::text::LayoutJob::single_section(
        name.to_string(),
        egui::TextFormat::simple(egui::FontId::proportional(ui::SMALL_PT), color::text()),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (tail_left - 8.0 - (rect.left() + 22.0)).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = p.layout_job(job);
    let cut = galley.elided;
    p.galley(
        egui::pos2(rect.left() + 22.0, y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    if cut {
        resp.on_hover_text(name.as_ref());
    }
}

#[cfg(test)]
mod tests {
    use crate::app::{OndinApp, TopMenu};
    use crate::library::state::Library;
    use eframe::egui;
    use ondin_core::kurbo::Size;
    use ondin_core::{NodeKind, Operation, Transaction};

    fn painted(ctx: &egui::Context, app: &mut OndinApp) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..2 {
            out = ctx
                .run_ui(Default::default(), |_| app.publish_popover(ctx))
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

    /// **A kit publishes from its popover, and what it published is in the file**
    /// (§5.3e; §15 D1015). Never published, the count is every component
    /// and the popover's verb names it; publishing bumps the mark to v1, stamps it,
    /// keeps the note, saves — so the library scan reads v1 from the file's first
    /// 4 KB — and leaves nothing to publish, which the next ask says instead of
    /// opening the popover. An edit after it is counted again.
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
        let doc_root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: main,
                parent: doc_root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(80.0, 30.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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
        assert!(app.publishing.is_some(), "the popover opens");
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

    /// While a compare is open, the kit's mark and its publish, written outside
    /// `apply`, are refused with the compare's sentence.
    /// flip: `make_kit` without its compare guard → fails at "Make a kit is refused".
    #[test]
    fn a_compare_refuses_the_kit_mark_and_publish() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let version = std::env::temp_dir().join(format!(
            "ondin-publish-compare-{}.ondin",
            std::process::id()
        ));
        std::fs::write(&version, ondin_core::io::save(&app.session.doc).unwrap()).unwrap();
        let main = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: main,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(80.0, 30.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Button".into()),
            },
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ])));
        app.compare_with(
            crate::compare::Pick {
                path: version.clone(),
                title: "Before".into(),
                when: None,
            },
            None,
        );
        assert!(app.session.compare.is_some(), "the compare opens");

        app.make_kit();
        assert!(
            app.session.doc.meta().kit.is_none(),
            "Make a kit is refused"
        );
        assert_eq!(app.session.status().text, crate::compare::READ_ONLY);

        let mut meta = app.session.doc.meta().clone();
        meta.kit = Some(ondin_core::meta::KitMark {
            sharing: true,
            ..Default::default()
        });
        app.session.doc.set_meta(meta);
        app.stop_sharing_kit();
        assert!(app.shared_kit().is_some(), "Stop sharing is refused");
        app.open_publish();
        assert!(
            app.publishing.is_none(),
            "the publish popover does not open"
        );
        app.publish_now("Then".into());
        assert_eq!(
            app.shared_kit().map(|k| k.version),
            Some(0),
            "nor a publish"
        );
        let _ = std::fs::remove_file(&version);
    }

    /// **A set's row counts its variants, not its children** (§15 D1015's fix):
    /// a set may hold a plain layer beside its variants, and the popover read
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
        let root = app.session.doc.first_page();
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 20.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
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
                    props: vec![VariantProp::new(
                        "Size",
                        vec!["Small".into(), "Large".into()]
                    )],
                    ..Default::default()
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

    /// The click that opens the popover — a menu row's — is not read as a click
    /// outside it; a later one is, and lets it go.
    /// flip: `dropdown_padded` without its `shown` gate → fails at "the opening click".
    #[test]
    fn the_opening_click_keeps_the_popover_and_a_later_one_closes_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let main = app.session.ids.mint();
        let page = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: main,
                parent: page,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(80.0, 30.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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
        let click = |at: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            egui::RawInput {
                events: vec![egui::Event::PointerMoved(at), press(true), press(false)],
                ..Default::default()
            }
        };
        let outside = egui::pos2(5.0, 5.0);
        app.open_publish();
        assert!(app.publishing.is_some(), "the fixture opens it");
        let _ = ctx.run_ui(click(outside), |_| app.publish_popover(&ctx));
        let _ = ctx.run_ui(Default::default(), |_| app.publish_popover(&ctx));
        assert!(
            app.publishing.is_some() && app.open_menu == TopMenu::Publish,
            "the opening click"
        );
        let _ = ctx.run_ui(click(outside), |_| app.publish_popover(&ctx));
        let _ = ctx.run_ui(Default::default(), |_| app.publish_popover(&ctx));
        assert!(app.publishing.is_none(), "a later click outside lets it go");
    }
}
