//! Kit updates in a consumer (§5.3e; §15 D1016).
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
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::kurbo::{Affine, Rect, Vec2};
use ondin_core::{NodeId, Operation};
use std::collections::{BTreeMap, HashSet};

/// The popover's width: room for a kit's slash-path component names. The
/// publish popover shares it.
pub(crate) const POP_W: f32 = 340.0;
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
    /// Told *Later* for this version.
    pub later: bool,
}

/// How often the kits a document uses are looked at on disk while it is open —
/// within the half-minute to minute the maintainer ruled (§15 D1020): every few
/// seconds would be nearer the moment a kit publishes, and was ruled too often.
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

/// The update popover's review of what to take: components unticked by their
/// vault top, kits opened or closed by hand, and a component's open menu.
/// Started afresh each time the popover opens.
#[derive(Default)]
pub(crate) struct KitReview {
    pub off: HashSet<NodeId>,
    /// A kit's table opened (`true`) or closed by hand; otherwise one kit
    /// opens and several start closed.
    open: BTreeMap<String, bool>,
    /// A component row's menu: its top, and where it was asked for.
    menu: Option<(NodeId, egui::Pos2)>,
}

impl KitReview {
    /// Whether `kit`'s table shows, when `n` kits wait.
    fn is_open(&self, kit: &str, n: usize) -> bool {
        self.open.get(kit).copied().unwrap_or(n == 1)
    }

    /// Tick or untick `top`, keeping the ticks whole under `needs`: ticking a
    /// top ticks what it needs, and unticking one unticks what needs it.
    pub(crate) fn toggle(&mut self, top: NodeId, needs: &[(NodeId, NodeId)]) {
        let mut stack = vec![top];
        if self.off.contains(&top) {
            while let Some(t) = stack.pop() {
                if self.off.remove(&t) {
                    stack.extend(needs.iter().filter(|(a, _)| *a == t).map(|(_, b)| *b));
                }
            }
        } else {
            while let Some(t) = stack.pop() {
                if self.off.insert(t) {
                    stack.extend(needs.iter().filter(|(_, b)| *b == t).map(|(a, _)| *a));
                }
            }
        }
    }
}

/// The popover's list at its tallest; past it, the list scrolls.
pub(crate) const LIST_MAX_H: f32 = 400.0;
/// What the popover holds besides its list — title, footer lines, buttons and
/// padding — taken from the room below the pill before the list's cap.
const POP_CHROME_H: f32 = 130.0;

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
    /// Bring the update pill when a kit this document uses publishes while
    /// it is open: checked every [`WATCH_EVERY`] seconds and whenever the window
    /// regains the focus (§15 D1020).
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

    /// The kits the vault holds mains of, and those whose variables or styles
    /// are held without one, each once.
    fn used_kit_ids(&self) -> Vec<String> {
        let doc = &self.session.doc;
        let tops: &[NodeId] = doc
            .vault()
            .and_then(|v| doc.get(v))
            .map_or(&[], |v| v.children());
        let mains = tops
            .iter()
            .filter_map(|t| doc.get(*t).and_then(|n| n.origin()))
            .map(|o| o.kit.as_str());
        let vars = doc
            .variables()
            .iter()
            .filter_map(|v| v.origin.as_ref().map(|o| o.kit.as_str()));
        let styles = doc
            .styles()
            .iter()
            .filter_map(|s| s.origin.as_ref().map(|o| o.kit.as_str()));
        let mut out: Vec<String> = Vec::new();
        for kit in mains.chain(vars).chain(styles) {
            if !out.iter().any(|k| k == kit) {
                out.push(kit.to_string());
            }
        }
        out
    }

    /// Every kit this document uses with a newer publish in the library — by a
    /// vendored main, or by variables or styles alone, held through another
    /// kit's main; such a kit's update is what that kit's own update and every
    /// insert from it wait on.
    ///
    /// `count` asks for each kit's instance count, which is a scan of the whole
    /// document per vendored main — the open popover's, never the pill's, which
    /// asks every frame.
    pub(crate) fn kits_waiting(&self, count: bool) -> Vec<Waiting> {
        let doc = &self.session.doc;
        let tops: &[NodeId] = doc
            .vault()
            .and_then(|v| doc.get(v))
            .map_or(&[], |v| v.children());
        let own = doc.meta().id.clone();
        let library = kits_in(&self.library, own.as_deref());
        let later = own
            .as_ref()
            .and_then(|d| self.library.local.kit_later.get(d))
            .cloned()
            .unwrap_or_default();
        let mut out: Vec<Waiting> = Vec::new();
        for top in tops {
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
        let tokens = doc
            .variables()
            .iter()
            .filter_map(|v| v.origin.as_ref().map(|o| (o.kit.as_str(), o.version)))
            .chain(
                doc.styles()
                    .iter()
                    .filter_map(|s| s.origin.as_ref().map(|o| (o.kit.as_str(), o.version))),
            );
        for (kit, version) in tokens {
            if tops.iter().any(|t| {
                doc.get(*t)
                    .and_then(|n| n.origin())
                    .is_some_and(|o| o.kit == kit)
            }) {
                continue;
            }
            let Some(k) = library.iter().find(|k| k.id == kit) else {
                continue;
            };
            match out.iter_mut().find(|w| w.kit.id == kit) {
                Some(w) => w.held = w.held.min(version),
                None => out.push(Waiting {
                    kit: k.clone(),
                    held: version,
                    mains: 0,
                    instances: 0,
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

    /// The pill beside the save state: *N kit updates*, or after
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
            ui::count(n, "kit update")
        };
        let scale = ui::Scale::Toolbar;
        let glyph_pt = scale.glyph_pt();
        let font = egui::FontId::proportional(scale.text_pt());
        let text_w = ui
            .ctx()
            .fonts_mut(|f| f.layout_no_wrap(text.clone(), font.clone(), color::text()))
            .size()
            .x;
        let w = 9.0 + glyph_pt + 6.0 + text_w + 9.0;
        // Latched while updates wait unreviewed or the popover is open; at rest once
        // they were put off with *Later*.
        let state = FieldButton::on_if(open || !quiet);
        let (rect, resp, fg) = ui::button_face(ui, egui::vec2(w, ui::TOOLBAR_H), state);
        let p = ui.painter();
        let y = rect.center().y;
        p.text(
            egui::pos2(rect.left() + 9.0, y),
            egui::Align2::LEFT_CENTER,
            icon::ARROW_CIRCLE_DOWN,
            theme::icon_font(glyph_pt),
            fg,
        );
        p.text(
            egui::pos2(rect.left() + 9.0 + glyph_pt + 6.0, y),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            fg,
        );
        let resp = resp.on_hover_text(if quiet {
            format!("{} · click to review", ui::count(n, "kit update"))
        } else {
            "Kits this document uses have published updates".to_string()
        });
        if resp.clicked() {
            // The published states the popover's Accept needs, loaded off the
            // frame while the user reads it.
            for w in &waiting {
                self.kit_shelf.ask(&w.kit);
            }
            if !open {
                self.kit_review = KitReview::default();
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
        // What accepting would change — once every publish is loaded.
        let summary = if ready {
            self.update_summary(&waiting)
        } else {
            None
        };
        // Every component row, by its top: its name and its kit's.
        let mut rows: Vec<(NodeId, String, String, usize)> = Vec::new();
        for k in summary.iter().flat_map(|s| &s.kits) {
            for c in &k.components {
                if !rows.iter().any(|r| r.0 == c.top) {
                    let kit = waiting.iter().find(|w| w.kit.id == k.kit);
                    let kit = kit.map_or_else(String::new, |w| w.kit.name.clone());
                    rows.push((c.top, c.name.clone(), kit, c.instances));
                }
            }
        }
        let needs = summary.as_ref().map_or(&[][..], |s| &s.needs[..]).to_vec();
        // A publish read while the popover is open can change the rows: what
        // is unticked is only ever a row on show.
        if summary.is_some() {
            self.kit_review
                .off
                .retain(|t| rows.iter().any(|r| r.0 == *t));
        }
        // A kit with no component rows — its variables and styles alone — is
        // taken whatever is ticked.
        let bare = summary.as_ref().is_some_and(|s| {
            waiting.iter().any(|w| {
                s.kits
                    .iter()
                    .find(|k| k.kit == w.kit.id)
                    .is_none_or(|k| k.components.is_empty())
            })
        });
        let total = rows.len();
        let taken = rows
            .iter()
            .filter(|r| !self.kit_review.off.contains(&r.0))
            .count();
        let list_max = ui::popover_list_max(&ctx, anchor.y, POP_CHROME_H, LIST_MAX_H);
        let pad = ui::POPOVER_PAD;
        self.dropdown_padded(
            &ctx,
            "kit-updates",
            anchor,
            POP_W,
            pad,
            Some(&resp),
            |app, ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(
                    egui::RichText::new(format!(
                        "{} {} updates",
                        ui::count(n, "kit"),
                        if n == 1 { "has" } else { "have" }
                    ))
                    .size(ui::CONTROL_PT)
                    .color(theme::text::strong()),
                );
                ui.add_space(4.0);
                let off = app.kit_review.off.clone();
                let row_of = |t: NodeId| rows.iter().find(|r| r.0 == t);
                let name = |t: NodeId| row_of(t).map(|r| r.1.clone());
                // An unticked row whose update needs another unticked one says so.
                let why = |t: NodeId| {
                    if !off.contains(&t) {
                        return None;
                    }
                    let (_, u) = needs.iter().find(|(a, b)| *a == t && off.contains(b))?;
                    let (me, other) = (row_of(t)?, row_of(*u)?);
                    let tail = match me.2 == other.2 {
                        true => format!("needs {}", other.1),
                        false => format!("needs {} from {}", other.1, other.2),
                    };
                    Some((
                        tail,
                        format!(
                            "{}'s update uses {}'s — tick {} to take both",
                            me.1, other.1, me.1
                        ),
                    ))
                };
                let mut clicked: Option<(String, Option<super::summary::ReviewAct>)> = None;
                // As tall as its rows, up to the cap or the room below the pill.
                ui::fit_scroll(ui, "kit-updates-list", list_max, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for w in &waiting {
                        let part = summary
                            .as_ref()
                            .and_then(|s| s.kits.iter().find(|k| k.kit == w.kit.id));
                        let review = super::summary::Review {
                            off: &off,
                            why: &why,
                            name: &name,
                            held: w.held,
                            version: w.kit.version,
                        };
                        let opened = app.kit_review.is_open(&w.kit.id, n);
                        match waiting_row(ui, w, part, opened, &review) {
                            Some(RowClick::Fold) => clicked = Some((w.kit.id.clone(), None)),
                            Some(RowClick::Review(a)) => {
                                clicked = Some((w.kit.id.clone(), Some(a)));
                            }
                            None => {}
                        }
                    }
                });
                match clicked {
                    Some((kit, None)) => {
                        let was = app.kit_review.is_open(&kit, n);
                        app.kit_review.open.insert(kit, !was);
                    }
                    Some((_, Some(super::summary::ReviewAct::Toggle(t)))) => {
                        app.kit_review.toggle(t, &needs);
                        app.kit_review.menu = None;
                    }
                    Some((_, Some(super::summary::ReviewAct::Menu(t, at)))) => {
                        app.kit_review.menu = Some((t, at));
                    }
                    None => {}
                }
                // The instances the ticked components reach; before the dry run
                // has read them, every instance of the kits.
                let instances: usize = match summary {
                    Some(_) => rows
                        .iter()
                        .filter(|r| !app.kit_review.off.contains(&r.0))
                        .map(|r| r.3)
                        .sum(),
                    None => waiting.iter().map(|w| w.instances).sum(),
                };
                ui.add_space(4.0);
                // The layers the kits' removals detach keep their values.
                if let Some((n, one)) = summary.as_ref().map(|s| s.detaching(&app.kit_review.off))
                    && n > 0
                {
                    let layers = ui::count(n, "layer");
                    ui.label(
                        egui::RichText::new(match one {
                            Some(name) => format!("{layers} using {name} keep its values."),
                            None => {
                                format!("{layers} using what the kits removed keep their values.")
                            }
                        })
                        .size(ui::SMALL_PT)
                        .color(theme::text::dim()),
                    );
                }
                ui.label(
                    egui::RichText::new(format!(
                        "Changes {}. Your overrides stay.",
                        ui::count(instances, "instance")
                    ))
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
                );
                ui.add_space(6.0);
                // Every component ticked reads *Accept all*, as before the review.
                let accept = match (ready, taken) {
                    (false, _) => "Loading…".to_string(),
                    (true, _) if taken == total => "Accept all".to_string(),
                    (true, 0) => "Accept".to_string(),
                    (true, _) => format!("Accept {taken} of {total}"),
                };
                let takes = taken > 0 || total == 0 || bare;
                let state = match ready && takes {
                    true => crate::ui::FieldButton::Primary,
                    false => crate::ui::FieldButton::Disabled,
                };
                let why = (ready && !takes).then_some("Tick a component to take its update");
                let buttons = [
                    ("Later", crate::ui::FieldButton::Off),
                    (accept.as_str(), state),
                ];
                match crate::settings::footer_row_noted(ui, &buttons, why) {
                    Some(0) => {
                        app.open_menu = TopMenu::None;
                        app.later_kit_updates();
                    }
                    Some(_) => {
                        app.open_menu = TopMenu::None;
                        let held: Vec<NodeId> = app.kit_review.off.iter().copied().collect();
                        app.accept_kit_updates_holding(None, &held);
                    }
                    None => {}
                }
            },
        );
        self.component_menu(&ctx);
    }

    /// A review row's menu (right-click on a component): *Select instances*,
    /// which closes the popover for the canvas, and *Go to main in ‹kit›*.
    fn component_menu(&mut self, ctx: &egui::Context) {
        let Some((top, at)) = self.kit_review.menu else {
            return;
        };
        if self.open_menu != TopMenu::KitUpdates {
            self.kit_review.menu = None;
            return;
        }
        let doc = &self.session.doc;
        let mains = match doc.get(top) {
            Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, top),
            Some(_) => vec![top],
            None => {
                self.kit_review.menu = None;
                return;
            }
        };
        let instances: Vec<NodeId> = mains
            .iter()
            .flat_map(|m| ondin_core::component::instances_of(doc, *m))
            .filter(|i| !doc.in_vault(*i))
            .collect();
        let kit = doc
            .get(top)
            .and_then(|n| n.origin())
            .and_then(|o| {
                self.kits_waiting(false)
                    .into_iter()
                    .find(|w| w.kit.id == o.kit)
            })
            .map_or_else(|| "the kit".to_string(), |w| w.kit.name);
        let go = format!("Go to main in {kit}");
        let count = instances.len().to_string();
        let w = 220.0;
        let size = egui::vec2(w, 2.0 * ui::MENU_ROW_H + 2.0 * ui::MENU_PAD);
        let pos = ui::menu_place(ctx, at, size);
        let mut act = None;
        let rect = egui::Area::new(Self::flyout_id("kit-updates"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::menu_frame(ui::MENU_PAD).show(ui, |ui| {
                    ui.set_width(ui::menu_inner_w(w, ui::MENU_PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    let r = ui::MenuRow::new(icon::SELECTION_ALL, "Select instances")
                        .accel(Some(count.as_str()))
                        .enabled(!instances.is_empty());
                    if ui::menu_row(ui, r, ui::MENU_ROW_H).clicked() {
                        act = Some(0);
                    }
                    let r = ui::MenuRow::new(icon::ARROW_UP_RIGHT, &go);
                    if ui::menu_row(ui, r, ui::MENU_ROW_H).clicked() {
                        act = Some(1);
                    }
                });
            })
            .response
            .rect;
        match act {
            Some(0) => {
                self.kit_review.menu = None;
                self.open_menu = TopMenu::None;
                self.select_font_users(instances);
            }
            Some(_) => {
                self.kit_review.menu = None;
                self.open_menu = TopMenu::None;
                self.go_to_kit_main_of(top);
            }
            None => {
                let away = ctx.input(|i| {
                    i.pointer.any_pressed()
                        && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
                });
                // The press that opened it is no press away from it.
                let opening = ctx.input(|i| i.pointer.secondary_clicked());
                if (away && !opening) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.kit_review.menu = None;
                }
            }
        }
    }

    /// Open the update popover from elsewhere than its pill — a kit header's
    /// *v8 → v9* in the variables browser — asking the shelf for the publishes its
    /// summary and *Accept all* read, as the pill's click does.
    pub(crate) fn open_kit_updates(&mut self) {
        if self.open_menu == TopMenu::KitUpdates {
            return;
        }
        for w in &self.kits_waiting(false) {
            self.kit_shelf.ask(&w.kit);
        }
        self.kit_review = KitReview::default();
        self.toggle_menu(TopMenu::KitUpdates);
    }

    /// What accepting the `waiting` kits would change, from a dry run of
    /// `kit::update_all` over the shelf's publishes — kept until the document
    /// or a waiting version moves. `None` while a publish is not loaded, or
    /// when the dry run fails (the accept then says why).
    fn update_summary(&mut self, waiting: &[Waiting]) -> Option<super::summary::Summary> {
        let key: super::summary::Key = (
            self.session.revision(),
            waiting
                .iter()
                .map(|w| (w.kit.id.clone(), w.kit.version))
                .collect(),
        );
        if let Some((k, s)) = &self.kit_summary
            && *k == key
        {
            return s.clone();
        }
        let mut loaded = Vec::new();
        for w in waiting {
            loaded.push((
                w.kit.id.clone(),
                w.kit.version,
                self.kit_shelf.current(&w.kit)?,
            ));
        }
        let args: Vec<(&str, u32, &ondin_core::kit::Published)> = loaded
            .iter()
            .map(|(k, v, p)| (k.as_str(), *v, p.as_ref()))
            .collect();
        // The dry run mints from the session's ids: ids are never reused, so
        // the ones it spends are only skipped.
        let page = self.session.page;
        let s = ondin_core::kit::update_all(&self.session.doc, &mut self.session.ids, page, &args)
            .ok()
            .map(|u| super::summary::summary(&self.session.doc, &u));
        self.kit_summary = Some((key, s.clone()));
        s
    }

    /// *Later*: remember the versions waiting now, so the pill shrinks and
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
    /// transaction. Mains a kit removed are made local and stacked at the
    /// top right of the content (D1011 (3)); the toast says what changed and that
    /// overrides were kept, and names `Ctrl+Z` (D981 (d): toasts are text).
    pub(crate) fn accept_kit_updates(&mut self, only: Option<&str>) {
        self.accept_kit_updates_holding(only, &[]);
    }

    /// [`Self::accept_kit_updates`] with the review's unticked components,
    /// by their vault tops, left as they are (`kit::update_holding`). A kit
    /// whose every component is held is skipped whole, its variables and
    /// styles too; the toast names what stayed behind.
    pub(crate) fn accept_kit_updates_holding(&mut self, only: Option<&str>, held: &[NodeId]) {
        // The typing is committed before the update, so a layer being typed
        // into is touched — cut loose rather than deleted under the session,
        // which then committed onto a node that was gone and said "Text
        // updated".
        self.finish_text_first();
        // The review's rows, read off the summary it was drawn from.
        let parts: Vec<super::summary::KitSummary> = self
            .kit_summary
            .as_ref()
            .and_then(|(_, s)| s.as_ref())
            .map(|s| s.kits.clone())
            .unwrap_or_default();
        let skipped = |kit: &str| {
            !held.is_empty()
                && parts.iter().find(|k| k.kit == kit).is_some_and(|k| {
                    let tops = k.tops();
                    !tops.is_empty() && tops.iter().all(|t| held.contains(t))
                })
        };
        let (waiting, skipped): (Vec<Waiting>, Vec<Waiting>) = self
            .kits_waiting(false)
            .into_iter()
            .filter(|w| only.is_none_or(|k| k == w.kit.id))
            .partition(|w| !skipped(&w.kit.id));
        if waiting.is_empty() {
            return;
        }
        // The held components the toast names, with the version they stay at.
        let doc = &self.session.doc;
        let stayed: Vec<(String, u32)> = parts
            .iter()
            .flat_map(|k| &k.components)
            .filter(|c| held.contains(&c.top))
            .filter(|c| {
                !skipped.iter().any(|w| {
                    doc.get(c.top)
                        .and_then(|n| n.origin())
                        .is_some_and(|o| o.kit == w.kit.id)
                })
            })
            // One per top: a removed variant's row is its set's.
            .filter(|c| !matches!(c.change, super::summary::Change::Detached(_)))
            .filter_map(|c| Some((c.top, c.name.clone(), doc.get(c.top)?.origin()?.version)))
            .fold(Vec::new(), |mut v: Vec<(NodeId, String, u32)>, x| {
                if !v.iter().any(|y| y.0 == x.0) {
                    v.push(x);
                }
                v
            })
            .into_iter()
            .map(|(_, name, v)| (name, v))
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
                // Nothing is applied while one waiting kit cannot be read: kits
                // reach each other's mains (§15 D1022, D1024), so a partial
                // accept is the riskier answer — and the message says the
                // others wait too (§15 D1036 (5)).
                None => {
                    self.session.fail(format!(
                        "Nothing was updated, because {} could not be read",
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
        let page = self.session.page;
        let content = self.session.preview_world_bounds(page);
        let update =
            match ondin_core::kit::update_holding(doc, &mut self.session.ids, page, &args, held) {
                Ok(u) => u,
                // **This kit's publish reaches a newer version of a third kit's
                // main than the document holds** (§15 D1024): from the card or the
                // canvas menu, which accept one kit, it would fail for good. When more
                // updates wait, all of them are accepted together — the third kit's
                // among them — as *Accept all* does; otherwise the third kit is not in
                // the library to update from.
                // A third kit's modes held at another version than this publish
                // read are the same owed update (§15 D1064).
                Err(
                    ondin_core::kit::KitError::NeedsUpdate(_) | ondin_core::kit::KitError::Modes(_),
                ) if only.is_some() && self.kits_waiting(false).len() > waiting.len() => {
                    self.accept_kit_updates(None);
                    return;
                }
                // The review skipped a kit whose update this one leans on.
                Err(
                    ondin_core::kit::KitError::NeedsUpdate(_) | ondin_core::kit::KitError::Modes(_),
                ) if !skipped.is_empty() => {
                    let names: Vec<&str> = skipped.iter().map(|w| w.kit.name.as_str()).collect();
                    self.session.fail(format!(
                        "This update needs {}'s too, which the review leaves out — tick one of \
                         its components",
                        names.join(" and ")
                    ));
                    return;
                }
                Err(
                    ondin_core::kit::KitError::NeedsUpdate(_) | ondin_core::kit::KitError::Modes(_),
                ) => {
                    self.session.fail(
                        "This update needs a newer version of another kit it uses, which is not \
                     in your library"
                            .to_string(),
                    );
                    return;
                }
                // §15 D1036 (2): a kit built on this one.
                Err(ondin_core::kit::KitError::Circular(own)) => {
                    let own = match self.session.doc.get(own) {
                        Some(_) => crate::private::layer(&self.session.doc, own),
                        None => "component".into(),
                    };
                    self.session.fail(format!(
                        "This update uses this kit's own {own}, so it cannot be taken here"
                    ));
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
        // The instances a variant the kit removed takes with it, counted before
        // the commit detaches them (§15 D982 (c), D1036 (4)).
        let cut: Vec<(String, usize)> = update
            .removed
            .iter()
            .filter_map(|m| {
                doc.get(*m)?;
                let here = ondin_core::component::instances_of(doc, *m)
                    .into_iter()
                    .filter(|i| !doc.in_vault(*i))
                    .count();
                (here > 0).then(|| (crate::private::layer(doc, *m).into_owned(), here))
            })
            .collect();
        let mut tx = update.tx;
        let placed = stack_local(doc, &tx, &update.made_local, content);
        tx.0.extend(placed);
        self.session
            .name_step("Update kit", icon::ARROW_CIRCLE_DOWN);
        if !self.session.commit(tx) {
            return;
        }
        let doc = &self.session.doc;
        let kept = update
            .changed
            .iter()
            .flat_map(|m| match doc.get(*m) {
                Some(n) if n.set().is_some() => ondin_core::variant::variants(doc, *m),
                _ => vec![*m],
            })
            .flat_map(|m| ondin_core::component::instances_of(doc, m))
            .filter(|i| ondin_core::reset::drift(doc, *i).any())
            .count();
        let kits = waiting.len();
        // Only the mains that changed (§15 D1036 (5)): a version-only publish
        // moves every main's version and changes none of them.
        let changed = update.changed.len();
        // What the review left behind: the held components, the kits skipped.
        let behind = held_words(&stayed, &skipped);
        let mut toast = if changed == 0 {
            format!(
                "Updated {}{behind} · no component you use changed · Ctrl+Z to undo",
                ui::count(kits, "kit")
            )
        } else if stayed.is_empty() {
            format!(
                "Updated {} from {} · overrides kept on {}{behind} · Ctrl+Z to undo",
                ui::count(changed, "component"),
                ui::count(kits, "kit"),
                ui::count(kept, "instance"),
            )
        } else {
            format!(
                "Updated {changed} of {} from {} · overrides kept on {}{behind} · Ctrl+Z to undo",
                ui::count(changed + stayed.len(), "component"),
                ui::count(kits, "kit"),
                ui::count(kept, "instance"),
            )
        };
        for (name, n) in &cut {
            toast = format!(
                "{toast} · {} of {name} detached — the kit removed it",
                ui::count(*n, "instance")
            );
        }
        // And the kits' variables and styles (the notice's counts, §15 D1065): what came
        // new or changed, and what the kit removed — its users keeping the values.
        let tokens = |vars: usize, styles: usize| match (vars, styles) {
            (0, 0) => None,
            (v, 0) => Some(ui::count(v, "variable")),
            (0, s) => Some(ui::count(s, "style")),
            (v, s) => Some(format!(
                "{} and {}",
                ui::count(v, "variable"),
                ui::count(s, "style")
            )),
        };
        if let Some(t) = tokens(update.vars_changed.len(), update.styles_changed.len()) {
            toast = format!("{toast} · {t} updated");
        }
        if let Some(t) = tokens(update.vars_removed.len(), update.styles_removed.len()) {
            toast = format!("{toast} · {t} the kit removed detached, values kept");
        }
        if let Some(&first) = update.made_local.first().filter(|m| doc.get(**m).is_some()) {
            toast = match update.made_local.len() {
                1 => format!(
                    "{toast} · {} is now a local main, at the top right of the canvas",
                    crate::private::layer(doc, first)
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
/// is its children's — placed by its origin at height 0, a tall group would
/// overlap the next.
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
        // (§15 D874), and stacking by it overlapped the mains.
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

/// The partial accept's words, each after a `" · "`: *Button and Card stay at
/// v4*, *Charts skipped* — empty when the review held nothing back.
fn held_words(stayed: &[(String, u32)], skipped: &[Waiting]) -> String {
    let mut out = String::new();
    if let Some((first, v)) = stayed.first() {
        let names = match stayed {
            [_] => first.clone(),
            [a, b] => format!("{} and {}", a.0, b.0),
            [a, b, rest @ ..] => format!("{}, {} and {} more", a.0, b.0, rest.len()),
            [] => String::new(),
        };
        let verb = if stayed.len() == 1 { "stays" } else { "stay" };
        out += &match stayed.iter().all(|(_, x)| x == v) {
            true => format!(" · {names} {verb} at v{v}"),
            false => format!(" · {names} {verb} behind"),
        };
    }
    match skipped {
        [] => {}
        [w] => out += &format!(" · {} skipped", w.kit.name),
        _ => out += &format!(" · {} skipped", ui::count(skipped.len(), "kit")),
    }
    out
}

/// One kit in the popover: its caret, name and *v8 → v9*, the headline's
/// counts and its publish note — and, while `open`, the review table the dry
/// run reads (`kits::summary::draw`). Answers what was clicked.
fn waiting_row(
    ui: &mut egui::Ui,
    w: &Waiting,
    part: Option<&super::summary::KitSummary>,
    open: bool,
    review: &super::summary::Review<'_>,
) -> Option<RowClick> {
    // The caret folds the table, when there is one: the whole head is its
    // target, its ink brightening as a fold head's does.
    let table = part.is_some_and(|k| !k.components.is_empty() || k.counts() != (0, 0, 0));
    let sense = match table {
        true => egui::Sense::click(),
        false => egui::Sense::hover(),
    };
    let (rect, head) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ui::FOLD_HEAD_H), sense);
    let ink = match head.hovered() && table {
        true => theme::text::muted(),
        false => theme::text::faint(),
    };
    let p = ui.painter();
    let y = rect.center().y;
    let mut x = rect.left();
    if table {
        ui::fold_caret(p, egui::pos2(x, y), open, ink);
        x += 16.0;
    }
    let glyph = p.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        icon::PACKAGE,
        theme::icon_font(12.0),
        theme::text::muted(),
    );
    let version = p.text(
        egui::pos2(rect.right(), y),
        egui::Align2::RIGHT_CENTER,
        format!("v{} → v{}", w.held, w.kit.version),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    // The name is cut short of the version, and then names itself in a tooltip.
    let name_x = glyph.right() + 6.0;
    let mut job = egui::text::LayoutJob::single_section(
        w.kit.name.clone(),
        egui::TextFormat::simple(egui::FontId::proportional(ui::CONTROL_PT), color::text()),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (version.left() - 8.0 - name_x).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = p.layout_job(job);
    let cut = galley.elided;
    p.galley(
        egui::pos2(name_x, y - galley.size().y / 2.0),
        galley,
        color::text(),
    );
    let head = match cut {
        true => head.on_hover_text(&w.kit.name),
        false => head,
    };
    let mut click = head.clicked().then_some(RowClick::Fold);
    let line = match part.map(super::summary::headline) {
        Some(h) if !h.is_empty() => h,
        Some(_) => "No component you use changes".to_string(),
        None if w.mains == 0 => "Its variables and styles".to_string(),
        None => ui::count(w.mains, "component"),
    };
    // The headline and the note stand under the name, wrapping within the card.
    let indent = (name_x - rect.left()).round() as i8;
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: indent,
            ..Default::default()
        })
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(line)
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
            );
            if let Some(note) = &w.kit.note {
                ui.label(
                    egui::RichText::new(format!("“{}”", note.trim()))
                        .size(ui::SMALL_PT)
                        .italics()
                        .color(theme::text::muted()),
                );
            }
        });
    if let Some(k) = part
        && open
        && let Some(a) = super::summary::draw(ui, k, review)
    {
        click = Some(RowClick::Review(a));
    }
    click
}

/// What a kit's row in the popover was asked.
enum RowClick {
    /// Its head: open or close its table.
    Fold,
    Review(super::summary::ReviewAct),
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
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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
        let root_id = app.session.doc.first_page();
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

    /// **A kit held only through its variables waits for its update too**: its
    /// update is what the kit carrying it and every insert from it wait on.
    ///
    /// flip: `kits_waiting`'s held-tokens loop removed → fails *"the kit waits"*.
    #[test]
    fn a_kit_held_only_through_its_variables_waits() {
        use ondin_core::variable::{Cell, KitMode, KitModes, Mode, VarKind, VarOrigin, Variable};
        let ctx = egui::Context::default();
        let root = temp_root("tokens-only");
        let (path, kit, _, dot) = file_kit(&root);
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.to_path_buf());
        app.library = Library::open(root.to_path_buf());
        let mut doc = Document::new(app.session.ids.mint());
        let doc_path =
            crate::library::store::file_document(&root, None, "Landing", &mut doc).unwrap();
        app.library.refresh();
        app.open_path(&doc_path);
        let (mine, theirs, var) = (
            app.session.ids.mint_mode(),
            app.session.ids.mint_mode(),
            app.session.ids.mint_var(),
        );
        let held = Transaction(vec![
            Operation::AddMode {
                collection: ondin_core::variable::CollectionId::FIRST,
                mode: Mode {
                    id: mine,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::SetKitModes {
                kit: kit.clone(),
                modes: Some(KitModes {
                    name: Some("Atlas".into()),
                    modes: vec![KitMode {
                        id: theirs,
                        name: "Default".into(),
                        from: theirs,
                        collection: String::new(),
                    }],
                    map: Default::default(),
                }),
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "brand".into(),
                    kind: VarKind::Number,
                    cells: [(theirs, Cell::Number(1.0))].into(),
                    scope: None,
                    origin: Some(VarOrigin {
                        kit: kit.clone(),
                        var,
                        version: 4,
                    }),
                    forked: None,
                    description: String::new(),
                },
            },
        ]);
        assert!(app.session.commit(held));
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
        assert_eq!(w.len(), 1, "the kit waits");
        assert_eq!((w[0].held, w[0].kit.version, w[0].mains), (4, 5, 0));
    }

    fn settle_for_accept(app: &mut OndinApp) {
        for w in app.kits_waiting(false) {
            app.kit_shelf.ask(&w.kit);
        }
        app.kit_shelf.settle();
    }

    /// **A newer publish waits, *Later* quiets it, *Accept all* takes it in one
    /// step** (§5.3e; §15 D1016). The kit re-publishes with its dot
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

    /// **The review takes the ticked components and leaves the rest**: Badge
    /// and Pill both change; with Pill unticked the accept takes Badge alone,
    /// Pill keeps its version, the kit still waits, and the toast names what
    /// stayed.
    ///
    /// flip: `accept_kit_updates_holding` passing no held tops to
    /// `update_holding` → fails at "Pill keeps its version".
    #[test]
    fn a_review_takes_the_ticked_components_and_leaves_the_rest() {
        let ctx = egui::Context::default();
        let root = temp_root("review");
        let (path, kit, badge, dot) = file_kit(&root);
        let mut kids = IdSource::new(0xAD);
        let pill = kids.mint();
        let kit_root = ondin_core::io::load(&std::fs::read(&path).unwrap())
            .unwrap()
            .first_page();
        republish(
            &path,
            vec![
                Operation::CreateNode {
                    id: pill,
                    parent: kit_root,
                    index: 1,
                    kind: NodeKind::Artboard {
                        size: Size::new(30.0, 12.0),
                        corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: Some("Pill".into()),
                },
                Operation::SetComponent {
                    id: pill,
                    component: true,
                },
            ],
            "A pill",
        );
        let (mut app, inst) = consumer(&ctx, &root, &kit, badge);
        let Some(super::Load::Ready(Some(p))) = app.kit_shelf.load(&kit) else {
            panic!("the kit loads")
        };
        let page = app.session.doc.first_page();
        let (tx, _) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &kit,
            5,
            p,
            pill,
            page,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        settle_for_accept(&mut app);
        app.accept_kit_updates(None);
        assert!(app.kits_waiting(false).is_empty(), "both at v5");

        republish(
            &path,
            vec![
                Operation::SetOpacity {
                    id: dot,
                    opacity: 0.5,
                },
                Operation::SetOpacity {
                    id: pill,
                    opacity: 0.5,
                },
            ],
            "Softer",
        );
        // Past the first publish's time, so the shelf reads the file again.
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(6))
            .unwrap();
        drop(f);
        app.library.refresh();
        settle_for_accept(&mut app);
        let waiting = app.kits_waiting(false);
        let summary = app.update_summary(&waiting).expect("the dry run reads");
        let rows: Vec<(NodeId, String)> = summary.kits[0]
            .components
            .iter()
            .map(|c| (c.top, c.name.clone()))
            .collect();
        assert_eq!(rows.len(), 2, "{rows:?}");
        let pill_top = rows.iter().find(|r| r.1 == "Pill").unwrap().0;
        app.kit_review.toggle(pill_top, &summary.needs);
        let held: Vec<NodeId> = app.kit_review.off.iter().copied().collect();
        assert_eq!(held, [pill_top]);
        app.accept_kit_updates_holding(None, &held);

        let doc = &app.session.doc;
        let idot = doc.get(inst).unwrap().children()[0];
        assert_eq!(doc.get(idot).unwrap().opacity(), 0.5, "Badge is taken");
        let v = doc.get(pill_top).unwrap().origin().unwrap().version;
        assert_eq!(v, 5, "Pill keeps its version");
        let w = app.kits_waiting(false);
        assert_eq!((w.len(), w[0].held), (1, 5), "the kit still waits");
        let toast = app.session.status().text.clone();
        assert!(
            toast.contains("Updated 1 of 2 components") && toast.contains("Pill stays at v5"),
            "{toast}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Ticking a component ticks what it needs, and unticking one unticks what
    /// needs it, across a chain.
    #[test]
    fn review_ticks_stay_whole_under_needs() {
        let mut ids = IdSource::new(0xAE);
        let [a, b, c] = [(); 3].map(|_| ids.mint());
        // a needs b, b needs c.
        let needs = [(a, b), (b, c)];
        let mut r = super::KitReview::default();
        r.toggle(c, &needs);
        let off = |r: &super::KitReview| {
            let mut v: Vec<NodeId> = r.off.iter().copied().collect();
            v.sort();
            v
        };
        let mut all = vec![a, b, c];
        all.sort();
        assert_eq!(off(&r), all, "unticking c unticks b and a");
        r.toggle(b, &needs);
        assert_eq!(off(&r), [a], "ticking b ticks c, not a");
        r.toggle(a, &needs);
        assert!(r.off.is_empty());
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
    /// D1023): the library scanned the kit at v4, the kit published v5, and the
    /// shelf then read the file. Its content is v5's, so it is not the current
    /// publish of an entry that says v4 — an accept would write v5's mains
    /// labelled v4.
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
            .preview_world_bounds(app.session.doc.first_page())
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
            Some(doc.first_page()),
            "out of the vault"
        );
        assert!(doc.get(main).unwrap().origin().is_none());
        let at = doc.get(main).unwrap().transform().translation();
        assert_eq!((at.x, at.y), (content.x1 + super::LOCAL_GAP, content.y0));
        let toast = app.session.status().text.clone();
        assert!(toast.contains("Badge is now a local main"), "{toast}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A kit instance's card says where its main lives**: *Atlas kit
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

    /// ***Make local* from the card**: the vendored main is revealed down
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
        assert_eq!(doc.get(main).unwrap().parent(), Some(doc.first_page()));
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
    /// it** (§5.3e): *Up to date · v4*, then *v5 waiting* once the kit
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
    /// Flip, run: a group's height counted as 0 fails *"no overlap"*.
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
                corner_smoothing: 0.0,
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

    /// **Hugging frames stack by the boxes layout gives them**: two flex frames
    /// stored at 10×10 that each hug a 120×40 rect. Stacked by stored height,
    /// the second overlapped the first by 30.
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
                        corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
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
                        corner_smoothing: 0.0,
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

    /// ***Make local*'s card counts what follows the revealed main, and says
    /// what the kit keeps** (§15 D1036 (1)). The page holds an *Icon* and a
    /// *Card* nesting one; making *Icon* local reaches the page's *Icon* and not
    /// *Card*'s nested copy, which reaches it through the kit's *Card* and now
    /// keeps the kit's — and the kit's *Card* is the one component that keeps
    /// it. Splitting `instances_of` by `in_vault`, which it never lists, would
    /// count both instances and never draw the sentence.
    ///
    /// Flip, run: counting every instance fails *"the page's Icon alone"* with
    /// 2.
    #[test]
    fn make_local_counts_what_follows_and_what_the_kit_keeps() {
        let ctx = egui::Context::default();
        let mut ids = IdSource::new(0xA1);
        let r = ids.mint();
        let mut kit = Document::new(r);
        let [icon, dot, card] = [(); 3].map(|_| ids.mint());
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(20.0, 20.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        kit.apply(&Transaction(vec![
            frame(icon, r, 0, "Icon"),
            frame(dot, icon, 0, "Dot"),
            Operation::SetComponent {
                id: icon,
                component: true,
            },
            frame(card, r, 1, "Card"),
        ]))
        .unwrap();
        let (tx, _) = ondin_core::insert_subtrees(
            &kit,
            &mut ids,
            &[ondin_core::Placement {
                nodes: kit.capture_subtree(icon).unwrap(),
                parent: card,
                index: None,
            }],
            Default::default(),
        );
        kit.apply(&tx).unwrap();
        kit.apply(&Transaction(vec![Operation::SetComponent {
            id: card,
            component: true,
        }]))
        .unwrap();
        let published = ondin_core::kit::freeze(&kit).unwrap();
        let kit_id = "5eed00000000000000000000000000a1";

        let mut app = OndinApp::headless(&ctx);
        let root = app.session.doc.first_page();
        let mut placed = Vec::new();
        for main in [icon, card] {
            let (tx, inst) = ondin_core::kit::insert(
                &app.session.doc,
                &mut app.session.ids,
                kit_id,
                1,
                &published,
                main,
                root,
            )
            .unwrap();
            assert!(app.session.commit(tx));
            placed.push(inst);
        }
        let vault_icon = app.session.doc.get(placed[0]).unwrap().link().unwrap();
        assert_eq!(
            ondin_core::component::instances_of(&app.session.doc, vault_icon).len(),
            2,
            "fixture: the page's Icon, and Card's nested one"
        );
        assert_eq!(
            app.make_local_reach(vault_icon),
            (1, 1),
            "the page's Icon alone, and the kit's Card"
        );
    }

    /// **An accept that cannot read a waiting kit applies nothing, and says so**
    /// (§15 D1036 (5)): the kit publishes, and its file is gone before the
    /// accept. It said *"Atlas could not be read, so its update waits"*, which
    /// read as though the others had been taken.
    ///
    /// Flip, run: the old wording fails *"nothing was updated"*.
    #[test]
    fn an_accept_that_cannot_read_a_kit_says_nothing_was_updated() {
        let ctx = egui::Context::default();
        let root = temp_root("unreadable");
        let (path, kit, badge, dot) = file_kit(&root);
        let (mut app, _) = consumer(&ctx, &root, &kit, badge);
        republish(
            &path,
            vec![Operation::SetOpacity {
                id: dot,
                opacity: 0.5,
            }],
            "Softer dot",
        );
        app.library.refresh();
        assert_eq!(app.kits_waiting(false).len(), 1, "fixture: one waits");
        std::fs::remove_file(&path).unwrap();
        let before = app.session.doc.clone();
        app.accept_kit_updates(None);
        assert_eq!(app.session.doc, before);
        let said = app.session.status().text.clone();
        assert_eq!(
            said, "Nothing was updated, because Atlas could not be read",
            "nothing was updated"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **One kit's accept that reaches a newer main of another kit takes every
    /// waiting update, or says why it cannot** (§15 D1024 — *"the retry is not
    /// driven by a test"*). Kit *Cards* nests kit *Icons*' *Icon* in its
    /// *Card*; the consumer places *Card*, so it holds both at v1. *Icons*
    /// publishes v2 — a *Ring* added to *Icon*, which v1 lacks — and *Cards*'
    /// author takes it, through a commit so the nested copy gains the ring, and
    /// publishes v2 too. *Cards*' v2 alone
    /// cannot be applied — it reaches *Icons*' ring and the document holds *Icon*
    /// at v1 — so accepting *Cards* from its card or the canvas menu (`only`)
    /// accepts both, as *Accept all* does, in one undo step. With *Icons*' file
    /// gone there is nothing to accept it from: nothing is applied, and the
    /// message says what is missing.
    ///
    /// ⚠️ **A version-only or a field-only publish of *Icons* does not make the
    /// case**: `update_all` refuses only a node the held version lacks. The first
    /// cut softened the dot, and *Cards*' v2 alone applied cleanly.
    ///
    /// Flip, run: the retry arm deleted (its guard `false && …`) fails *"the
    /// retry took both"*, the fallback message read instead. Flip, run: the
    /// fallback arm's `session.fail` dropped fails *"says why"*, the status still
    /// the undo's *"Undo"*.
    #[test]
    fn one_kits_accept_needing_another_kits_update_takes_both_or_says_why() {
        let ctx = egui::Context::default();
        let root = temp_root("retry");
        let frame = |id, parent, index, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(20.0, 20.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        };
        let file = |doc: &mut Document, name: &str| {
            let path = crate::library::store::file_document(&root, None, name, doc).unwrap();
            let mut meta = doc.meta().clone();
            meta.kit = Some(KitMark::default());
            doc.set_meta(meta);
            ondin_core::kit::publish(doc, None, 1).unwrap();
            crate::library::store::write_document(&path, doc).unwrap();
            (path, doc.meta().id.clone().unwrap())
        };
        // *Icons*: a main *Icon* holding a dot.
        let mut ids = IdSource::new(0xB1);
        let mut icons = Document::new(ids.mint());
        let [icon, dot] = [(); 2].map(|_| ids.mint());
        let r = icons.first_page();
        icons
            .apply(&Transaction(vec![
                frame(icon, r, 0, "Icon"),
                frame(dot, icon, 0, "Dot"),
                Operation::SetComponent {
                    id: icon,
                    component: true,
                },
            ]))
            .unwrap();
        let (icons_path, icons_id) = file(&mut icons, "Icons");
        let icons_v1 = icons.published().unwrap().clone();
        // *Cards*: a main *Card* nesting an *Icon* of *Icons* v1.
        let mut aids = IdSource::new(0xA2);
        let mut cards = Document::new(aids.mint());
        let card = aids.mint();
        let r = cards.first_page();
        cards
            .apply(&Transaction(vec![
                frame(card, r, 0, "Card"),
                Operation::SetComponent {
                    id: card,
                    component: true,
                },
            ]))
            .unwrap();
        let (tx, _) =
            ondin_core::kit::insert(&cards, &mut aids, &icons_id, 1, &icons_v1, icon, card)
                .unwrap();
        cards.apply(&tx).unwrap();
        let (cards_path, cards_id) = file(&mut cards, "Cards");
        let cards_v1 = cards.published().unwrap().clone();

        let mut app = crate::kits::picker::tests::app_on(&ctx, root.clone());
        let page = app.session.doc.first_page();
        let (tx, inst) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &cards_id,
            1,
            &cards_v1,
            card,
            page,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        assert!(
            app.kits_waiting(false).is_empty(),
            "the fixture: up to date"
        );

        // *Icons* v2 — a layer added, which v1 lacks — then *Cards* v2 built on it.
        let ring = ids.mint();
        republish(&icons_path, vec![frame(ring, icon, 1, "Ring")], "A ring");
        // Its author takes the update — through a commit, so *Card*'s nested
        // copy gains the ring — and publishes.
        let mut author = OndinApp::headless(&ctx);
        author.prefs.base_folder = Some(root.clone());
        author.library = Library::open(root.clone());
        author.open_path(&cards_path);
        author.accept_kit_updates(None);
        assert!(
            author.kits_waiting(false).is_empty(),
            "the fixture: Cards took Icons v2"
        );
        author.open_publish();
        author.publish_now("On the ringed icon".into());
        let f = std::fs::File::options()
            .write(true)
            .open(&cards_path)
            .unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(4))
            .unwrap();
        let cards = ondin_core::io::load(&std::fs::read(&cards_path).unwrap()).unwrap();
        assert_eq!(
            cards.meta().kit.as_ref().map(|k| k.version),
            Some(2),
            "the fixture: Cards v2 on disk"
        );
        let cards_v2 = cards.published().unwrap().clone();
        app.library.refresh();
        assert_eq!(app.kits_waiting(false).len(), 2, "the fixture: both wait");
        let mut scratch_ids = IdSource::new(0xA4);
        assert!(
            matches!(
                ondin_core::kit::update_all(
                    &app.session.doc,
                    &mut scratch_ids,
                    app.session.page,
                    &[(&cards_id, 2, &cards_v2)]
                ),
                Err(ondin_core::kit::KitError::NeedsUpdate(_))
            ),
            "the fixture: Cards' v2 alone needs Icons' v2"
        );

        let before = app.session.doc.clone();
        app.accept_kit_updates(Some(&cards_id));
        assert!(
            app.kits_waiting(false).is_empty(),
            "the retry took both: {}",
            app.session.status().text
        );
        let doc = &app.session.doc;
        let nested = doc.get(inst).unwrap().children()[0];
        assert_eq!(
            doc.get(nested).unwrap().children().len(),
            2,
            "Icons' v2 arrived: the ring"
        );
        app.undo();
        assert_eq!(app.session.doc, before, "in one undo step");

        // *Icons* gone from the library: Cards waits alone, and cannot be taken.
        std::fs::remove_file(&icons_path).unwrap();
        app.library.refresh();
        assert_eq!(app.kits_waiting(false).len(), 1, "the fixture: Cards alone");
        app.accept_kit_updates(Some(&cards_id));
        assert_eq!(app.session.doc, before, "nothing applied");
        assert_eq!(
            app.session.status().text,
            "This update needs a newer version of another kit it uses, which is not in your \
             library",
            "says why"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***Make all local* reveals a set and a main as one undo step, stacked**
    /// (§5.3e, 6B — *"undoable"*).
    /// The page holds an instance of *Atlas*' set *Button* and one of its
    /// *Badge*, so the kit's vault holds two tops. The dialog is driven — its
    /// *Make all local* clicked — and both tops leave the vault for the page,
    /// stacked down the right without overlapping, the toast counting two; one
    /// undo puts the document back whole.
    ///
    /// Flip, run: `make_local_now` calling itself once per top when handed
    /// several — two commits — fails before the undo is reached, at the toast:
    /// *"Badge is now a local main, at the top right of the canvas"*, the second
    /// commit's own (each stacked against the content the first left, so the
    /// overlap assertion passes it). Flip, run: the `stack_local` placements left
    /// out fails *"stacked, not overlapping"*, both at the origin.
    #[test]
    fn make_all_local_reveals_a_set_and_a_main_in_one_step() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("all-local");
        let (kit, set, badge) = crate::kits::picker::tests::file_kit(&root);
        let mut app = crate::kits::picker::tests::app_on(&ctx, root.clone());
        let published = ondin_core::io::load(&std::fs::read(root.join("atlas.ondin")).unwrap())
            .unwrap()
            .published()
            .unwrap()
            .clone();
        let page = app.session.doc.first_page();
        for top in [set, badge] {
            let (tx, _) = ondin_core::kit::insert(
                &app.session.doc,
                &mut app.session.ids,
                &kit,
                4,
                &published,
                top,
                page,
            )
            .unwrap();
            assert!(app.session.commit(tx));
        }
        let tops = app.kits_used()[0].tops.clone();
        assert_eq!(tops.len(), 2, "the fixture: a set and a main in the vault");
        let before = app.session.doc.clone();

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |_| app.make_all_local_dialog(&ctx),
            )
        };
        app.making_all_local = Some(kit.clone());
        run(&mut app, vec![]);
        let out = run(&mut app, vec![]);
        let at = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Make all local" => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("the dialog's button");
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut app, vec![egui::Event::PointerMoved(at)]);
        run(&mut app, vec![press(true)]);
        run(&mut app, vec![press(false)]);
        assert!(app.making_all_local.is_none(), "the dialog closed");

        let doc = &app.session.doc;
        for t in &tops {
            assert_eq!(
                doc.get(*t).unwrap().parent(),
                Some(doc.first_page()),
                "both revealed"
            );
        }
        assert!(app.kits_used().is_empty(), "the kit is off the list");
        let res = ondin_core::Resolved::rebuild(doc);
        let (a, b) = (
            res.world_bounds(tops[0]).unwrap(),
            res.world_bounds(tops[1]).unwrap(),
        );
        assert!(
            a.x1 <= b.x0 || b.x1 <= a.x0 || a.y1 <= b.y0 || b.y1 <= a.y0,
            "stacked, not overlapping: {a:?} {b:?}"
        );
        let toast = app.session.status().text.clone();
        assert!(
            toast.starts_with("2 components are now local mains"),
            "{toast}"
        );
        app.undo();
        assert_eq!(app.session.doc, before, "one undo step");
        let _ = std::fs::remove_dir_all(&root);
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

    /// **The kit doors end a live text session first**. Typing into a text layer
    /// of the kit opened by *Go to main in the kit*, then leaving by the back
    /// link, lost the typing: the save on the way out wrote `session.doc`, which
    /// holds none of it.
    /// The back link and *Kits used… → Open kit* both call `open_path`, which is
    /// where this drives it; the seam is `OndinApp::park_active`, which every
    /// change of tab passes.
    ///
    /// Flip, run: the `finish_text_first` line out of `park_active` fails *"the kit
    /// on disk keeps the typing"* with `Some("hello")`.
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

    /// ***Accept all* under a live text session commits the typing first**: the
    /// update is applied to a document that holds what is on screen, and no
    /// session is left over a document the update rewrote.
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
        let page = app.session.doc.first_page();
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

    /// ***Publish* under a live text session publishes what is on screen**: the
    /// published main's text reads the typing, and the kit has nothing left to
    /// publish after. Before, the frozen state read "hello", the version
    /// was spent, and ending the session left one change behind.
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

    /// *Make local*'s status tail (§15 D1066): the forks counted, and a frame
    /// set to a kit's mode named — empty when it forked nothing.
    #[test]
    fn make_locals_status_says_what_it_forked() {
        use crate::kits::card::forked_note;
        assert_eq!(forked_note((0, 0), 0), "");
        assert_eq!(
            forked_note((2, 1), 0),
            "; 2 variables and 1 style copied from the kit as this document's own"
        );
        assert_eq!(
            forked_note((1, 0), 3),
            "; 1 variable copied from the kit as this document's own; 3 frames set to a \
             kit's mode may draw differently"
        );
    }
}
