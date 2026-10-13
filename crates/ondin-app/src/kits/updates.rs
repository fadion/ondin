//! Kit updates in a consumer (§5.3e, 5A–5C; §15 D1016).
//!
//! A document notices a newer publish of a kit it vendors from by comparing what
//! the library scan says each kit last published (`meta.kit.version`, read from
//! the file's first 4 KB) with the versions its vendored mains carry — that scan
//! kept fresh for the kits it uses while the document is open, by a look at their
//! files every 45 seconds and on the window's focus (`watch_kits`, §15 D1020). A pill
//! beside the save state says how many kits wait, its popover what each brings,
//! and *Accept all* takes them in one undoable transaction
//! (`ondin_core::kit::update_all`) — the commit's main-edit passes carrying the
//! change to the instances. *Later* shrinks the pill to its glyph and count, and
//! it does not grow back until a kit publishes again.

use super::index::{KitEntry, Load, kits_in};
use crate::app::{OndinApp, TopMenu};
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::kurbo::{Affine, Rect, Vec2};
use ondin_core::{NodeId, Operation};

/// The popover's width.
const POP_W: f32 = 300.0;
/// The gap between the content's right edge and a main made local, and between
/// mains stacked there (D1011 (3)).
const LOCAL_GAP: f64 = 80.0;

/// One kit with a newer publish than this document's copies.
#[derive(Clone, Debug)]
pub(crate) struct Waiting {
    pub kit: KitEntry,
    /// The version this document's copies are at — the oldest, if they differ.
    pub held: u32,
    /// How many vendored mains and sets of it this document holds.
    pub mains: usize,
    /// How many instances of them this document has.
    pub instances: usize,
    /// Told *Later* for this version (5C).
    pub later: bool,
}

/// How often the kits a document uses are looked at on disk while it is open —
/// within the half-minute to minute the maintainer ruled (§15 D1020): every few
/// seconds would be nearer the design's *"the moment a kit publishes"*, and was
/// ruled too often.
pub(crate) const WATCH_EVERY: f64 = 45.0;

/// When the kits a document uses were last looked at (§15 D1020).
#[derive(Default)]
pub(crate) struct KitWatch {
    /// The input clock's time of the next look; 0 before the first.
    next: f64,
    /// Whether the window had the focus last frame — a look is due the moment it
    /// comes back, as the kit's author has most likely just published in another
    /// window.
    focused: bool,
}

/// A file's modification time, in the scan's whole seconds, and its size — what
/// the library scan read of it (`library::scan`), so the two compare. `None`
/// when it cannot be read.
fn on_disk(path: &std::path::Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    let modified = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some((modified, m.len()))
}

impl OndinApp {
    /// Bring the update pill (5A) when a kit this document uses publishes while
    /// it is open — the design's *"the moment a kit publishes"*, met every
    /// [`WATCH_EVERY`] seconds and whenever the window regains the focus (§15
    /// D1020; until then it waited for the library's next rescan, D1016).
    ///
    /// **A look is a `stat` of the used kits' files, not a scan**: the library is
    /// rescanned only when one of them changed size or modification time, or has
    /// gone (moved, so the scan finds it again by its id). A kit the library could
    /// not find is looked for on a focus only, that being a whole scan. Nothing
    /// happens for a document that uses no kit.
    pub(crate) fn watch_kits(&mut self, ctx: &egui::Context) {
        let (now, focused) = ctx.input(|i| (i.time, i.viewport().focused.unwrap_or(true)));
        if let Some(wait) = self.watch_kits_at(now, focused) {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// [`Self::watch_kits`] at the clock reading `now`, answering how long until
    /// the next look — `None` when there is nothing to watch.
    pub(crate) fn watch_kits_at(&mut self, now: f64, focused: bool) -> Option<f64> {
        let regained = focused && !self.kit_watch.focused;
        self.kit_watch.focused = focused;
        let used = self.used_kit_ids();
        if used.is_empty() {
            return None;
        }
        if now < self.kit_watch.next && !regained {
            return Some(self.kit_watch.next - now);
        }
        self.kit_watch.next = now + WATCH_EVERY;
        let own = self.session.doc.meta().id.clone();
        let library = kits_in(&self.library, own.as_deref());
        let mut stale = false;
        for id in &used {
            let Some(k) = library.iter().find(|k| k.id == *id) else {
                stale |= regained;
                continue;
            };
            let read = self.library.entry_at(&k.path).map(|e| (e.modified, e.size));
            stale |= on_disk(&k.path) != read;
        }
        if stale {
            self.library.refresh();
        }
        Some(WATCH_EVERY)
    }

    /// The kits the vault holds mains of, each once.
    fn used_kit_ids(&self) -> Vec<String> {
        let doc = &self.session.doc;
        let Some(vault) = doc.vault().and_then(|v| doc.get(v)) else {
            return Vec::new();
        };
        let mut out: Vec<String> = Vec::new();
        for o in vault
            .children()
            .iter()
            .filter_map(|t| doc.get(*t).and_then(|n| n.origin()))
        {
            if !out.contains(&o.kit) {
                out.push(o.kit.clone());
            }
        }
        out
    }

    /// Every kit this document uses with a newer publish in the library.
    ///
    /// `count` asks for each kit's instance count, which is a scan of the whole
    /// document per vendored main — the open popover's, never the pill's, which
    /// asks every frame.
    pub(crate) fn kits_waiting(&self, count: bool) -> Vec<Waiting> {
        let doc = &self.session.doc;
        let Some(vault) = doc.vault().and_then(|v| doc.get(v)) else {
            return Vec::new();
        };
        let own = doc.meta().id.clone();
        let library = kits_in(&self.library, own.as_deref());
        let later = own
            .as_ref()
            .and_then(|d| self.library.local.kit_later.get(d))
            .cloned()
            .unwrap_or_default();
        let mut out: Vec<Waiting> = Vec::new();
        for top in vault.children() {
            let Some(o) = doc.get(*top).and_then(|n| n.origin()) else {
                continue;
            };
            let Some(k) = library.iter().find(|k| k.id == o.kit) else {
                continue;
            };
            let instances: usize = if count {
                let mains: Vec<NodeId> = match doc.get(*top) {
                    Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, *top),
                    _ => vec![*top],
                };
                mains
                    .iter()
                    .map(|m| ondin_core::component::instances_of(doc, *m).len())
                    .sum()
            } else {
                0
            };
            match out.iter_mut().find(|w| w.kit.id == o.kit) {
                Some(w) => {
                    w.held = w.held.min(o.version);
                    w.mains += 1;
                    w.instances += instances;
                }
                None => out.push(Waiting {
                    kit: k.clone(),
                    held: o.version,
                    mains: 1,
                    instances,
                    later: false,
                }),
            }
        }
        out.retain(|w| w.kit.version > w.held);
        for w in &mut out {
            w.later = later.get(&w.kit.id).is_some_and(|v| *v >= w.kit.version);
        }
        out.sort_by_key(|w| w.kit.name.to_lowercase());
        out
    }

    /// The pill beside the save state (5A, 5C): *N kit updates*, or after
    /// *Later* its glyph and count; its popover summarises them.
    pub(crate) fn kit_update_pill(&mut self, ui: &mut egui::Ui) {
        let open = self.open_menu == TopMenu::KitUpdates;
        let waiting = self.kits_waiting(open);
        if waiting.is_empty() {
            if self.open_menu == TopMenu::KitUpdates {
                self.open_menu = TopMenu::None;
            }
            return;
        }
        let n = waiting.len();
        let quiet = waiting.iter().all(|w| w.later);
        let text = if quiet {
            n.to_string()
        } else {
            format!("{n} kit update{}", if n == 1 { "" } else { "s" })
        };
        let galley = ui
            .ctx()
            .fonts_mut(|f| f.layout_no_wrap(text, egui::FontId::proportional(11.5), color::TEXT));
        let w = 9.0 + 13.0 + 6.0 + galley.size().x + 9.0;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::click());
        let p = ui.painter();
        let ground = if open || resp.hovered() {
            color::text_a(48)
        } else if quiet {
            color::text_a(18)
        } else {
            color::text_a(36)
        };
        p.rect_filled(rect, egui::CornerRadius::same(6), ground);
        let y = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 9.0, y),
            egui::Align2::LEFT_CENTER,
            icon::ARROW_CIRCLE_DOWN,
            theme::icon_font(13.0),
            color::text_a(191),
        );
        p.galley(
            egui::pos2(rect.left() + 28.0, y - galley.size().y / 2.0),
            galley,
            color::TEXT,
        );
        let resp = resp.on_hover_text(if quiet {
            format!(
                "{n} kit update{} · click to review",
                if n == 1 { "" } else { "s" }
            )
        } else {
            "Kits this document uses have published updates".to_string()
        });
        if resp.clicked() {
            // The published states the popover's Accept needs, loaded off the
            // frame while the user reads it.
            for w in &waiting {
                self.kit_shelf.ask(&w.kit);
            }
            self.toggle_menu(TopMenu::KitUpdates);
        }
        if self.open_menu != TopMenu::KitUpdates {
            return;
        }
        let ctx = ui.ctx().clone();
        self.kit_shelf.pass(&ctx);
        let anchor = egui::pos2(rect.left(), rect.bottom() + crate::ui::MENU_GAP);
        // Waiting only on a read in flight: a kit the shelf could not load is
        // read again by the accept itself, which reports what it cannot read.
        let ready = waiting
            .iter()
            .all(|w| !matches!(self.kit_shelf.load(&w.kit.id), Some(Load::Pending)));
        self.dropdown(&ctx, "kit-updates", anchor, POP_W, &resp, |app, ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            ui.label(
                egui::RichText::new(format!(
                    "{n} kit{} {} updates",
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "has" } else { "have" }
                ))
                .size(12.0)
                .color(theme::text::STRONG),
            );
            ui.add_space(4.0);
            for w in &waiting {
                waiting_row(ui, w);
            }
            let instances: usize = waiting.iter().map(|w| w.instances).sum();
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "Changes {instances} instance{}. Your overrides stay.",
                    if instances == 1 { "" } else { "s" }
                ))
                .size(11.0)
                .color(theme::text::DIM),
            );
            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let accept = crate::settings::text_button(
                    ui,
                    if ready { "Accept all" } else { "Loading…" },
                    if ready {
                        crate::ui::FieldButton::On
                    } else {
                        crate::ui::FieldButton::Disabled
                    },
                );
                if accept.clicked() && ready {
                    app.open_menu = TopMenu::None;
                    app.accept_kit_updates(None);
                }
                if crate::settings::text_button(ui, "Later", crate::ui::FieldButton::Off).clicked()
                {
                    app.open_menu = TopMenu::None;
                    app.later_kit_updates();
                }
            });
        });
    }

    /// *Later* (5C): remember the versions waiting now, so the pill shrinks and
    /// stays shrunk until a kit publishes past them.
    pub(crate) fn later_kit_updates(&mut self) {
        let Some(doc) = self.session.doc.meta().id.clone() else {
            return;
        };
        let waiting = self.kits_waiting(false);
        let entry = self.library.local.kit_later.entry(doc).or_default();
        for w in waiting {
            entry.insert(w.kit.id.clone(), w.kit.version);
        }
        self.library.local.save();
    }

    /// Accept the waiting updates — every kit's, or only `only`'s — in one
    /// transaction (5B). Mains a kit removed are made local and stacked at the
    /// top right of the content (D1011 (3)); the toast says what changed and that
    /// overrides were kept, and names `Ctrl+Z` (D981 (d): toasts are text).
    pub(crate) fn accept_kit_updates(&mut self, only: Option<&str>) {
        // The typing is committed before the update, so a layer being typed
        // into is touched — cut loose rather than deleted under the session,
        // which then committed onto a node that was gone and said "Text
        // updated" (`[X3.2-L5-01]`).
        self.finish_text_first();
        let waiting: Vec<Waiting> = self
            .kits_waiting(false)
            .into_iter()
            .filter(|w| only.is_none_or(|k| k == w.kit.id))
            .collect();
        let mut loaded = Vec::new();
        for w in &waiting {
            // The shelf's copy when it has one; otherwise the file, read here —
            // an accept is a click, and the card's *Accept kit updates* arrives
            // with nothing asked for beforehand.
            //
            // Read here, it records **the version the file says**, not the
            // scan's, which a publish since may have passed (§15 D1023).
            let p = match self.kit_shelf.current(&w.kit) {
                Some(p) => Some((w.kit.version, p)),
                None => std::fs::read(&w.kit.path)
                    .ok()
                    .and_then(|b| ondin_core::io::load(&b).ok())
                    .and_then(|d| {
                        let version = d.meta().kit.as_ref().map(|k| k.version)?;
                        Some((version, std::sync::Arc::new(d.published()?.clone())))
                    }),
            };
            match p {
                Some((version, p)) => loaded.push((w.kit.id.clone(), version, p)),
                None => {
                    self.session.fail(format!(
                        "{} could not be read, so its update waits",
                        w.kit.name
                    ));
                    return;
                }
            }
        }
        let args: Vec<(&str, u32, &ondin_core::kit::Published)> = loaded
            .iter()
            .map(|(k, v, p)| (k.as_str(), *v, p.as_ref()))
            .collect();
        let doc = &self.session.doc;
        let content = self.session.preview_world_bounds(doc.root());
        let update = match ondin_core::kit::update_all(doc, &mut self.session.ids, &args) {
            Ok(u) => u,
            // **This kit's publish reaches a newer version of a third kit's
            // main than the document holds** (§15 D1024, the kits QA review's
            // second pass): from the card or the canvas menu, which accept one
            // kit, it failed for good. When more updates wait, all of them are
            // accepted together — the third kit's among them — as *Accept all*
            // does; otherwise the third kit is not in the library to update from.
            Err(ondin_core::kit::KitError::NeedsUpdate(_))
                if only.is_some() && self.kits_waiting(false).len() > waiting.len() =>
            {
                self.accept_kit_updates(None);
                return;
            }
            Err(ondin_core::kit::KitError::NeedsUpdate(_)) => {
                self.session.fail(
                    "This update needs a newer version of another kit it uses, which is not \
                     in your library"
                        .to_string(),
                );
                return;
            }
            Err(_) => {
                self.session.fail(
                    "The kit update could not be applied — its published components are \
                     incomplete"
                        .to_string(),
                );
                return;
            }
        };
        let mut tx = update.tx;
        let placed = stack_local(doc, &tx, &update.made_local, content);
        tx.0.extend(placed);
        if !self.session.commit(tx) {
            return;
        }
        let doc = &self.session.doc;
        let kept = update
            .updated
            .iter()
            .flat_map(|m| match doc.get(*m) {
                Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, *m),
                _ => vec![*m],
            })
            .flat_map(|m| ondin_core::component::instances_of(doc, m))
            .filter(|i| ondin_core::reset::drift(doc, *i).any())
            .count();
        let kits = waiting.len();
        let mut toast = format!(
            "Updated {} component{} from {kits} kit{} · overrides kept on {kept} instance{} · \
             Ctrl+Z to undo",
            update.updated.len(),
            if update.updated.len() == 1 { "" } else { "s" },
            if kits == 1 { "" } else { "s" },
            if kept == 1 { "" } else { "s" },
        );
        if let Some(first) = update.made_local.first().and_then(|m| doc.get(*m)) {
            toast = match update.made_local.len() {
                1 => format!(
                    "{toast} · {} is now a local main, at the top right of the canvas",
                    first.name()
                ),
                n => format!(
                    "{toast} · {n} removed components are now local mains, at the top right"
                ),
            };
        }
        self.session.info(toast);
    }
}

/// The `SetTransform`s that stack mains made local down the right of the
/// document's content, `LOCAL_GAP` out and apart (D1011 (3)) — each main's own
/// box, top-left, at its place in the column.
///
/// **Measured on the document `tx` leaves**, laid out: until the transaction
/// lands the mains are in the vault, which nothing resolves, and a group's box
/// is its children's — the first cut placed a group by its origin at height 0, so
/// a tall one overlapped the next (`arch-scribe`'s reading of D1016).
pub(crate) fn stack_local(
    doc: &ondin_core::Document,
    tx: &ondin_core::Transaction,
    mains: &[NodeId],
    content: Option<Rect>,
) -> Vec<Operation> {
    if mains.is_empty() {
        return Vec::new();
    }
    let mut scratch = doc.clone();
    if scratch.apply(tx).is_err() {
        return Vec::new();
    }
    let res = ondin_core::Resolved::rebuild(&scratch);
    let start = content.map_or(ondin_core::kurbo::Point::ZERO, |c| {
        ondin_core::kurbo::Point::new(c.x1 + LOCAL_GAP, c.y0)
    });
    let mut y = start.y;
    let mut out = Vec::new();
    for m in mains {
        let Some(n) = scratch.get(*m) else { continue };
        let at = n.transform();
        // Laid out, a frame too: a hugging frame's stored size is not its box
        // (§15 D874), and stacking by it overlapped the mains (the v0.5.0
        // release review's `[R1-L2-01]`).
        let Some(b) = res.world_bounds(*m) else {
            continue;
        };
        let shift = Vec2::new(start.x - b.x0, y - b.y0);
        out.push(Operation::SetTransform {
            id: *m,
            transform: Affine::translate(shift) * at,
        });
        y += b.height() + LOCAL_GAP;
    }
    out
}

/// One kit in the popover: its name and *v8 → v9*, how many components, and
/// its publish note (5A).
fn waiting_row(ui: &mut egui::Ui, w: &Waiting) {
    ui.horizontal(|ui| {
        ui.label(theme::icon_text(icon::PACKAGE, 12.0, theme::text::MUTED));
        ui.label(
            egui::RichText::new(&w.kit.name)
                .size(12.0)
                .color(color::TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("v{} → v{}", w.held, w.kit.version))
                    .size(10.5)
                    .color(theme::text::FAINT),
            );
        });
    });
    ui.label(
        egui::RichText::new(format!(
            "{} component{}",
            w.mains,
            if w.mains == 1 { "" } else { "s" }
        ))
        .size(11.0)
        .color(theme::text::DIM),
    );
    if let Some(note) = &w.kit.note {
        ui.label(
            egui::RichText::new(format!("“{note}”"))
                .size(11.0)
                .italics()
                .color(theme::text::MUTED),
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::app::OndinApp;
    use crate::library::state::Library;
    use eframe::egui;
    use ondin_core::kurbo::Size;
    use ondin_core::meta::KitMark;
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};
    use std::path::{Path, PathBuf};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-updates-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A kit *Atlas* in `root`, published at v4: one main *Badge* holding a dot.
    fn file_kit(root: &Path) -> (PathBuf, String, NodeId, NodeId) {
        let mut ids = IdSource::new(0xAC);
        let r = ids.mint();
        let mut doc = Document::new(r);
        let [badge, dot] = [(); 2].map(|_| ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: badge,
                parent: r,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(40.0, 20.0),
                },
                transform: None,
                name: Some("Badge".into()),
            },
            Operation::CreateNode {
                id: dot,
                parent: badge,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(4.0, 4.0),
                },
                transform: None,
                name: Some("Dot".into()),
            },
            Operation::SetComponent {
                id: badge,
                component: true,
            },
        ]))
        .unwrap();
        let path = crate::library::store::file_document(root, None, "Atlas", &mut doc).unwrap();
        let mut meta = doc.meta().clone();
        meta.kit = Some(KitMark {
            version: 3,
            ..KitMark::default()
        });
        doc.set_meta(meta);
        ondin_core::kit::publish(&mut doc, None, 1).unwrap();
        crate::library::store::write_document(&path, &doc).unwrap();
        (path, doc.meta().id.clone().unwrap(), badge, dot)
    }

    /// Edit the kit file with `ops` and publish it again, as its author would.
    fn republish(path: &Path, ops: Vec<Operation>, note: &str) {
        let mut doc = ondin_core::io::load(&std::fs::read(path).unwrap()).unwrap();
        doc.apply(&Transaction(ops)).unwrap();
        ondin_core::kit::publish(&mut doc, Some(note.into()), 2).unwrap();
        crate::library::store::write_document(path, &doc).unwrap();
        // A modification time a second on, so the shelf reads the file again
        // whatever the filesystem's resolution (the CLAUDE.md mtime lesson).
        let f = std::fs::File::options().write(true).open(path).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
    }

    /// A consumer with an instance of the kit's Badge, and the app open on it.
    fn consumer(ctx: &egui::Context, root: &Path, kit: &str, badge: NodeId) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.to_path_buf());
        app.library = Library::open(root.to_path_buf());
        let mut doc = Document::new(app.session.ids.mint());
        let path = crate::library::store::file_document(root, None, "Landing", &mut doc).unwrap();
        app.library.refresh();
        app.open_path(&path);
        let k = super::super::index::kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit)
            .unwrap();
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let Some(super::Load::Ready(Some(p))) = app.kit_shelf.load(kit) else {
            panic!("the kit loads")
        };
        let root_id = app.session.doc.root();
        let (tx, inst) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            kit,
            4,
            p,
            badge,
            root_id,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        (app, inst)
    }

    fn settle_for_accept(app: &mut OndinApp) {
        for w in app.kits_waiting(false) {
            app.kit_shelf.ask(&w.kit);
        }
        app.kit_shelf.settle();
    }

    /// **A newer publish waits, *Later* quiets it, *Accept all* takes it in one
    /// step** (§5.3e, 5A–5C; §15 D1016). The kit re-publishes with its dot
    /// changed; the consumer sees one kit waiting, v4 → v5, with the note; *Later*
    /// keeps it waiting but quiet; *Accept all* rewrites the vendored main and the
    /// instance follows, nothing waits, the toast says so, and one undo takes it
    /// back.
    #[test]
    fn a_newer_publish_waits_and_accept_all_takes_it() {
        let ctx = egui::Context::default();
        let root = temp_root("accept");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        assert!(app.kits_waiting(false).is_empty(), "up to date");

        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "Softer dot",
        );
        app.library.refresh();
        let w = app.kits_waiting(true);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].held, w[0].kit.version), (4, 5));
        assert_eq!(w[0].kit.note.as_deref(), Some("Softer dot"));
        assert_eq!((w[0].mains, w[0].instances), (1, 1));
        assert!(!w[0].later);

        app.later_kit_updates();
        assert!(app.kits_waiting(false)[0].later, "quiet, still waiting");

        settle_for_accept(&mut app);
        let before = app.session.doc.clone();
        app.accept_kit_updates(None);
        let idot = app.session.doc.get(inst).unwrap().children()[0];
        assert_eq!(
            app.session.doc.get(idot).unwrap().opacity(),
            0.5,
            "the instance follows"
        );
        assert!(app.kits_waiting(false).is_empty(), "nothing waits");
        let toast = app.session.status().text.clone();
        assert!(
            toast.contains("Updated 1 component from 1 kit") && toast.contains("Ctrl+Z"),
            "{toast}"
        );
        app.undo();
        assert_eq!(app.session.doc, before, "one undo");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A kit that publishes while its consumer is open brings the pill without
    /// a rescan from anywhere else** (§15 D1020): a look every `WATCH_EVERY`
    /// seconds `stat`s the kit's file and rescans the library when it changed.
    /// Between looks nothing is read; a window regaining the focus looks at once.
    ///
    /// Flip, run: the look never rescanning (`stale` never set) fails *"due: the
    /// pill"*.
    #[test]
    fn a_publish_while_open_is_noticed_by_the_next_look() {
        use super::WATCH_EVERY;
        let ctx = egui::Context::default();
        let root = temp_root("watch");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, _) = consumer(&ctx, &root, &kit, badge);

        assert_eq!(app.watch_kits_at(0.0, true), Some(WATCH_EVERY), "a look");
        assert!(app.kits_waiting(false).is_empty(), "up to date");

        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "Softer dot",
        );
        assert_eq!(app.watch_kits_at(10.0, true), Some(WATCH_EVERY - 10.0));
        assert!(app.kits_waiting(false).is_empty(), "not due: nothing read");
        app.watch_kits_at(WATCH_EVERY, true);
        assert_eq!(app.kits_waiting(false).len(), 1, "due: the pill");

        // Away and back: the look comes with the focus, not the clock.
        app.watch_kits_at(WATCH_EVERY + 1.0, false);
        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.25,
            }],
            "Softer still",
        );
        app.watch_kits_at(WATCH_EVERY + 2.0, true);
        let w = app.kits_waiting(false);
        assert_eq!(w[0].kit.version, 6, "the focus looked");

        // A document using no kit watches nothing.
        app.undo();
        assert_eq!(app.watch_kits_at(1000.0, true), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A publish read past the scan's version is not passed off as it** (§15
    /// D1023, the kits QA review's finding): the library scanned the kit at v4,
    /// the kit published v5, and the shelf then read the file. Its content is
    /// v5's, so it is not the current publish of an entry that says v4 — an
    /// accept would have written v5's mains labelled v4.
    ///
    /// Flip, run: `current` without its version comparison fails *"not v4's"*.
    #[test]
    fn a_publish_read_after_the_scan_is_not_the_scans_version() {
        let ctx = egui::Context::default();
        let root = temp_root("race");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, _) = consumer(&ctx, &root, &kit, badge);
        let scanned = super::kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit)
            .unwrap();
        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "Softer dot",
        );
        // A shelf that has never read it, asked with the scan's entry.
        app.kit_shelf = Default::default();
        app.kit_shelf.ask(&scanned);
        app.kit_shelf.settle();
        assert!(app.kit_shelf.current(&scanned).is_none(), "not v4's");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A component the kit removed is made local at the top right of the
    /// content** (D1011 (3)): its copy leaves the vault as an ordinary main, set
    /// `LOCAL_GAP` to the right of everything else and level with its top, its
    /// instance still linked, and the toast says where it went.
    #[test]
    fn a_removed_component_lands_local_at_the_top_right() {
        let ctx = egui::Context::default();
        let root = temp_root("local");
        let (path, kit, badge, _) = file_kit(&root);
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        let content = app
            .session
            .preview_world_bounds(app.session.doc.root())
            .unwrap();
        republish(
            &path,
            vec![Operation::DeleteNode { id: badge }],
            "Badge retired",
        );
        app.library.refresh();
        settle_for_accept(&mut app);
        app.accept_kit_updates(None);

        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        let doc = &app.session.doc;
        assert_eq!(
            doc.get(main).unwrap().parent(),
            Some(doc.root()),
            "out of the vault"
        );
        assert!(doc.get(main).unwrap().origin().is_none());
        let at = doc.get(main).unwrap().transform().translation();
        assert_eq!((at.x, at.y), (content.x1 + super::LOCAL_GAP, content.y0));
        let toast = app.session.status().text.clone();
        assert!(toast.contains("Badge is now a local main"), "{toast}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A kit instance's card says where its main lives** (4A, 4B): *Atlas kit
    /// · v4*, and *· v5 waiting* once the kit publishes again — painted, read off
    /// the frame. Its own *Accept kit updates* takes its kit **with nothing loaded
    /// beforehand**: the accept reads the kit's file itself when the shelf has
    /// not.
    #[test]
    fn a_kit_instance_names_its_kit_and_accepts_its_own_update() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("card");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        let line = app.kit_line(main).expect("a kit instance");
        assert_eq!(
            (line.kit_name.as_str(), line.version, line.waiting),
            ("Atlas", 4, None)
        );
        assert_eq!(
            app.kit_line(inst),
            None,
            "the instance itself is no kit's main"
        );

        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "",
        );
        app.library.refresh();
        let line = app.kit_line(main).unwrap();
        assert_eq!(line.waiting, Some(5));
        let painted: Vec<String> = ctx
            .run_ui(Default::default(), |ui| {
                super::super::card::source_line(ui, &line)
            })
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(
            painted.contains(&"Atlas kit · v4 · v5 waiting".to_string()),
            "{painted:?}"
        );

        app.kit_shelf = Default::default();
        app.accept_kit_updates(Some(&kit));
        assert!(
            app.kits_waiting(false).is_empty(),
            "accepted from the file itself"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***Make local* from the card** (4D/4E): the vendored main is revealed down
    /// the right of the content, the instance still linked, the toast saying
    /// where; one undo. And *Go to main in the kit* opens the kit with the main
    /// selected there and leaves the way back, which leaving the kit ends.
    #[test]
    fn make_local_and_go_to_main_in_the_kit() {
        let ctx = egui::Context::default();
        let root = temp_root("local-go");
        let (path, kit, badge, _) = file_kit(&root);
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        let consumer_path = app.session.path.clone().unwrap();

        // Go to main, then back.
        app.session.selection.set_one(inst);
        app.go_to_kit_main();
        assert_eq!(
            app.session.path.as_deref(),
            Some(path.as_path()),
            "the kit opened"
        );
        assert_eq!(app.session.selection.ids(), &[badge], "its main selected");
        assert_eq!(
            app.kit_return.as_ref().map(|(p, n)| (p.clone(), n.clone())),
            Some((consumer_path.clone(), "Landing".to_string()))
        );
        app.open_path(&consumer_path);
        assert!(app.kit_return.is_none(), "leaving the kit ends the link");

        // Make local.
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        let before = app.session.doc.clone();
        app.make_local_now(&[main]);
        let doc = &app.session.doc;
        assert_eq!(doc.get(main).unwrap().parent(), Some(doc.root()));
        assert_eq!(doc.get(inst).unwrap().link(), Some(main));
        assert!(
            app.session
                .status()
                .text
                .contains("Badge is now a local main")
        );
        assert!(app.kit_line(main).is_none(), "no kit's any more");
        app.undo();
        assert_eq!(app.session.doc, before, "one undo");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***Kits used…* lists each kit with its status, and *Make all local* empties
    /// it** (§5.3e, 6A/6B): *Up to date · v4*, then *v5 waiting* once the kit
    /// publishes, then *Unavailable* — still named, from what its copies recorded —
    /// once the file is gone; the dialog paints its title and the kit; making the
    /// kit's components local takes it off the list.
    #[test]
    fn kits_used_lists_the_status_and_make_all_local_empties_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("used");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, _) = consumer(&ctx, &root, &kit, badge);
        let used = app.kits_used();
        assert_eq!(used.len(), 1);
        assert_eq!(
            (
                used[0].name.as_str(),
                used[0].location.as_str(),
                used[0].instances
            ),
            ("Atlas", "Library", 1)
        );
        assert_eq!(used[0].status(), "Up to date · v4");

        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "",
        );
        app.library.refresh();
        assert_eq!(app.kits_used()[0].status(), "v5 waiting");

        app.kits_used_open = true;
        let mut painted = Vec::new();
        for _ in 0..2 {
            painted = ctx
                .run_ui(Default::default(), |_| app.kits_used_dialog(&ctx))
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<String>>();
        }
        assert!(
            painted.contains(&"Kits used in Landing".to_string()),
            "{painted:?}"
        );
        assert!(painted.contains(&"v5 waiting".to_string()), "{painted:?}");

        std::fs::remove_file(&path).unwrap();
        app.library.refresh();
        let used = app.kits_used();
        assert_eq!(
            (used[0].name.as_str(), used[0].status().as_str()),
            ("Atlas", "Unavailable")
        );

        let tops = used[0].tops.clone();
        app.make_local_now(&tops);
        assert!(app.kits_used().is_empty(), "off the list");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **An accept never takes a stale copy** (§15 D1016's fix (a)): the shelf
    /// read the kit at v4 (the fixture's insert did), the kit publishes v5, and the
    /// card's *Accept kit updates* — which asks the shelf nothing first — must
    /// bring v5's change, not write v4's content under v5's number.
    ///
    /// Flip, run: the accept taking `load`'s `Ready` copy whatever its file's
    /// time fails *"v5's change arrived"*, the dot still at full opacity.
    #[test]
    fn an_accept_never_takes_a_stale_copy() {
        let ctx = egui::Context::default();
        let root = temp_root("stale");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        assert!(
            matches!(app.kit_shelf.load(&kit), Some(super::Load::Ready(Some(_)))),
            "the fixture: the shelf holds v4"
        );
        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "",
        );
        app.library.refresh();
        app.accept_kit_updates(Some(&kit));
        let idot = app.session.doc.get(inst).unwrap().children()[0];
        assert_eq!(
            app.session.doc.get(idot).unwrap().opacity(),
            0.5,
            "v5's change arrived"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Mains made local stack without overlapping, groups included** (§15
    /// D1016's fix (c)): two groups, each a 30-tall square, are stacked down the
    /// right of the content — each by its own laid-out box, `LOCAL_GAP` apart.
    ///
    /// Flip, run: a group's height counted as 0, as the first cut did, fails
    /// *"no overlap"*.
    #[test]
    fn mains_made_local_stack_without_overlapping() {
        let mut ids = IdSource::new(0xC0);
        let r = ids.mint();
        let mut doc = Document::new(r);
        let [g1, s1, g2, s2] = [(); 4].map(|_| ids.mint());
        let square = |id, parent| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(30.0, 30.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let group = |id, index| Operation::CreateNode {
            id,
            parent: r,
            index,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            group(g1, 0),
            square(s1, g1),
            group(g2, 1),
            square(s2, g2),
        ]))
        .unwrap();
        let content = ondin_core::kurbo::Rect::new(0.0, 0.0, 100.0, 100.0);
        let ops = super::stack_local(&doc, &Transaction(vec![]), &[g1, g2], Some(content));
        doc.apply(&Transaction(ops)).unwrap();
        let res = ondin_core::Resolved::rebuild(&doc);
        let (b1, b2) = (res.world_bounds(g1).unwrap(), res.world_bounds(g2).unwrap());
        assert_eq!((b1.x0, b1.y0), (100.0 + super::LOCAL_GAP, 0.0));
        assert!(
            b2.y0 >= b1.y1 + super::LOCAL_GAP - 1e-9,
            "no overlap: {b1:?} {b2:?}"
        );
    }

    /// **Hugging frames stack by the boxes layout gives them** (the v0.5.0
    /// release review's `[R1-L2-01]`): two flex frames stored at 10×10 that each
    /// hug a 120×40 rect. Stacked by stored height, the second overlapped the
    /// first by 30.
    ///
    /// Flip, run: measuring a frame by its stored size again fails *"no
    /// overlap"*.
    #[test]
    fn hugging_mains_made_local_stack_without_overlapping() {
        use ondin_core::container::{Dimension, Display, Flex, LayoutItem};
        let mut ids = IdSource::new(0xDC);
        let r = ids.mint();
        let mut doc = Document::new(r);
        let item = LayoutItem {
            width: Dimension::FitContent,
            height: Dimension::FitContent,
            ..Default::default()
        };
        let mut frames = Vec::new();
        for i in 0..2 {
            let [f, c] = [(); 2].map(|_| ids.mint());
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: f,
                    parent: r,
                    index: i,
                    kind: NodeKind::Artboard {
                        size: Size::new(10.0, 10.0),
                    },
                    transform: None,
                    name: None,
                },
                Operation::CreateNode {
                    id: c,
                    parent: f,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(120.0, 40.0),
                        corner_radii: Default::default(),
                    },
                    transform: None,
                    name: None,
                },
                Operation::SetDisplay {
                    id: f,
                    display: Some(Display::Flex(Flex::default())),
                },
                Operation::SetLayoutItem { id: f, item },
            ]))
            .unwrap();
            frames.push(f);
        }
        let ops = super::stack_local(&doc, &Transaction(vec![]), &frames, None);
        doc.apply(&Transaction(ops)).unwrap();
        let res = ondin_core::Resolved::rebuild(&doc);
        let (b1, b2) = (
            res.world_bounds(frames[0]).unwrap(),
            res.world_bounds(frames[1]).unwrap(),
        );
        assert_eq!(b1.height(), 40.0, "fixture: the frames hug");
        assert!(
            b2.y0 >= b1.y1 + super::LOCAL_GAP - 1e-9,
            "no overlap: {b1:?} {b2:?}"
        );
    }

    /// A text layer reading "hello" under `parent`, committed and saved, and a
    /// live session over it holding "TYPED" the document does not.
    fn typing_into(app: &mut OndinApp, parent: NodeId) -> NodeId {
        let id = app.session.ids.mint();
        let parts = crate::fonts::FontService::default_text_parts();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Text {
                content: "hello".into(),
                style: Box::new(parts.style),
                spans: parts.spans,
                para_spans: parts.para_spans,
                paragraph: parts.paragraph,
                block: parts.block,
                sizing: parts.sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: Some("Label".into()),
        }])));
        assert!(app.save_file(false));
        app.begin_edit_text(Some(id), None);
        app.text
            .as_mut()
            .expect("the session is live")
            .editor
            .insert("TYPED");
        assert_eq!(
            text_of(&app.session.doc, id).as_deref(),
            Some("hello"),
            "fixture: the typing is the session's alone"
        );
        id
    }

    fn text_of(doc: &Document, id: NodeId) -> Option<String> {
        match doc.get(id)?.kind() {
            NodeKind::Text { content, .. } => Some(content.clone()),
            _ => None,
        }
    }

    /// **The kit doors end a live text session first** (`[X3.2-L5-01]`, the
    /// v0.5.0 release review's Critical). Typing into a text layer of the kit
    /// opened by *Go to main in the kit*, then leaving by the back link, lost the
    /// typing: `open_path` saved `session.doc`, which holds none of it, and
    /// `adopt_opened` dropped the session. The back link and *Kits used… → Open
    /// kit* both call `open_path`, which is where this drives it — the seam the
    /// fix is at, `may_replace_current_document`.
    ///
    /// Flip, run: the `finish_text_first` line out of
    /// `may_replace_current_document` fails *"the kit on disk keeps the typing"*
    /// with `Some("hello")`.
    #[test]
    fn leaving_the_kit_keeps_what_was_typed_in_it() {
        let ctx = egui::Context::default();
        let root = temp_root("text-leave");
        let (path, kit, badge, _) = file_kit(&root);
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        let consumer_path = app.session.path.clone().unwrap();
        app.session.selection.set_one(inst);
        app.go_to_kit_main();
        assert_eq!(app.session.path.as_deref(), Some(path.as_path()));
        let label = typing_into(&mut app, badge);

        app.open_path(&consumer_path);
        assert_eq!(app.session.path.as_deref(), Some(consumer_path.as_path()));
        let on_disk = ondin_core::io::load(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            text_of(&on_disk, label).as_deref(),
            Some("TYPED"),
            "the kit on disk keeps the typing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***Accept all* under a live text session commits the typing first**
    /// (`[X3.2-L5-01]`'s second route): the update is applied to a document that
    /// holds what is on screen, and no session is left over a document the
    /// update rewrote.
    ///
    /// Flip, run: the `finish_text_first` line out of `accept_kit_updates`
    /// fails *"the session ended before the update"* — the session still live
    /// after the accept.
    #[test]
    fn an_accept_commits_what_was_typed_first() {
        let ctx = egui::Context::default();
        let root = temp_root("text-accept");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, _) = consumer(&ctx, &root, &kit, badge);
        let page = app.session.doc.root();
        let label = typing_into(&mut app, page);
        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "Softer dot",
        );
        app.library.refresh();
        settle_for_accept(&mut app);
        app.accept_kit_updates(None);
        assert!(app.text.is_none(), "the session ended before the update");
        assert_eq!(text_of(&app.session.doc, label).as_deref(), Some("TYPED"));
        assert!(app.kits_waiting(false).is_empty(), "and the update landed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***Publish* under a live text session publishes what is on screen**
    /// (`[X5-L1-02]`): the published main's text reads the typing, and the kit
    /// has nothing left to publish after. Before, the frozen state read "hello",
    /// the version was spent, and ending the session left one change behind.
    ///
    /// Flip, run: the `finish_text_first` lines out of both `open_publish` and
    /// `publish_now` fail *"the publish holds the typing"* with `Some("hello")`.
    #[test]
    fn a_publish_holds_what_was_typed() {
        let ctx = egui::Context::default();
        let root = temp_root("text-publish");
        let (path, _, badge, _) = file_kit(&root);
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = Library::open(root.clone());
        app.open_path(&path);
        app.make_kit();
        let label = typing_into(&mut app, badge);

        app.open_publish();
        assert!(app.publishing.is_some(), "the dialog opened");
        app.publish_now("Words".into());
        let published = app.session.doc.published().expect("published");
        assert_eq!(
            text_of(published.document(), label).as_deref(),
            Some("TYPED"),
            "the publish holds the typing"
        );
        app.finish_text_first();
        assert_eq!(
            app.kit_changes().map(|c| c.count()),
            Some(0),
            "nothing left"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
