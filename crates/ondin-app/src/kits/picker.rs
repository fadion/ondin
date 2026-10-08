//! The floating insert picker (§5.3e, 3A–3I; §15 D1014).
//!
//! A panel beside the vertical toolbar, bottom-aligned with the hexagon at its
//! foot — `Shift+I` or the button opens it, `Escape` or the button closes it, and
//! nothing else closes or moves (3A). Browsing lists the kits this document uses
//! first, by last use, each open to its sets and lone mains grouped by the
//! top-level frame they sat in; then the library's other kits (3E). Typing turns
//! it into one flat list across every kit, best match first (3F). A click places
//! an instance at the viewport's centre; a tile dragged out of the panel and let
//! go over the canvas places it centred there, at the document's top level as a
//! click does — not into a frame under the pointer (§15 D1018).

use super::index::{KitEntry, Load, kits_in};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::kit::Published;
use ondin_core::kurbo::{Affine, Rect, Vec2};
use ondin_core::{Document, NodeId, NodeKind, Operation, Transaction};
use std::collections::HashSet;
use std::sync::Arc;

/// The panel's width, padding and corner, from the mockup (3A).
const W: f32 = 340.0;
const PAD: f32 = 12.0;
const RADIUS: u8 = 8;
/// The panel's height — the mockup's — whatever it holds: it does not grow as
/// a kit opens or shrink as a search narrows, so nothing under the pointer jumps.
/// Shorter only when the canvas has less room above the button than this.
const H: f32 = 460.0;
/// The search box's height and the gap below it.
const SEARCH_H: f32 = 30.0;
const SEARCH_GAP: f32 = 8.0;
/// A tile: three to a row, 52 tall, 8 apart across and 10 down (3E).
const COLS: usize = 3;
const TILE_H: f32 = 52.0;
const GAP_X: f32 = 8.0;
const GAP_Y: f32 = 10.0;
/// A list row's height — a kit header, a search result.
const ROW_H: f32 = 28.0;
/// How many results a search shows.
const MAX_RESULTS: usize = 40;

/// The picker's own state while it is open.
#[derive(Default)]
pub(crate) struct Picker {
    query: String,
    /// The kits shown open, by id — the first used kit opens with the picker.
    open: HashSet<String>,
    /// Whether the kits' open state has been seeded yet.
    seeded: bool,
    /// The highlighted search result, which `Enter` places.
    highlight: usize,
    /// Whether the search field should take the keyboard on the next frame —
    /// once, when the picker opens.
    focus: bool,
}

/// Where a placeable component comes from.
#[derive(Clone)]
enum Source {
    /// This document's own main — in its tree, or a copy in its vault of a kit
    /// that cannot be found (3I).
    Local(NodeId),
    /// A kit's published main or set.
    Kit {
        id: String,
        version: u32,
        published: Arc<Published>,
        top: NodeId,
    },
}

/// One thing the picker can place.
#[derive(Clone)]
struct Item {
    source: Source,
    name: String,
    /// A component set rather than a lone main — the tile's glyph.
    set: bool,
    /// The kit's name on a search row, and whether the document uses it yet.
    kit: Option<(String, bool)>,
    /// The texture the tile draws, when it has one.
    tile: Option<egui::TextureHandle>,
    /// Every name a search matches it by: its own, and a set's variants'.
    aliases: Vec<String>,
}

/// What a frame of the picker asked for.
enum Act {
    Place(Source),
    Toggle(String),
    /// 3G's *Open library*: the dashboard, on its *Kits* filter.
    OpenLibrary,
    /// A tile let go at this screen point — placed there when it is over the
    /// canvas and not over the panel itself.
    Drop(Source, egui::Pos2),
}

/// One kit as the picker lists it.
struct Shown {
    id: String,
    name: String,
    /// The right side of its header — *v8 · 24*, *not published*, *Unavailable*.
    status: String,
    used: bool,
    /// Its components, grouped by the top-level frame they sat in, in order.
    groups: Vec<(String, Vec<Item>)>,
    /// What the open kit says instead of tiles (3H, 3I), when it says something.
    note: Option<String>,
}

impl OndinApp {
    /// Open the picker with its search field taking the keyboard, or close it
    /// (`Shift+I`, the rail's hexagon). `Shift+I` reaches the close only while the
    /// field does not hold the keyboard — inside it the chord is a capital *I* —
    /// which is why `Escape` and the button are the ways out §5.3e names.
    pub(crate) fn toggle_kit_picker(&mut self) {
        self.kit_picker = match self.kit_picker {
            Some(_) => None,
            None => {
                // A rescan reads each file's first 4 KB, so a kit published since
                // the last one — in this window or synced in — is seen at once
                // rather than when the dashboard next looks (`kits::index`).
                self.library.refresh();
                Some(Picker {
                    focus: true,
                    ..Picker::default()
                })
            }
        };
    }

    /// Draw the picker, when it is open, and place what it was asked to.
    pub(crate) fn kit_picker_ui(&mut self, ctx: &egui::Context) {
        self.kit_shelf.pass(ctx);
        if self.kit_picker.is_none() {
            return;
        }
        // `Escape` is not read here: it is a rung of `OndinApp::escape`, so one
        // press closes one thing — a press inside the search field included,
        // since egui lets go of the field before the keymap reads the key.
        let own = self.session.doc.meta().id.clone();
        let library = kits_in(&self.library, own.as_deref());
        for k in &library {
            self.kit_shelf.ask(k);
        }
        let shown = self.shown_kits(ctx, &library);
        let local = self.local_items();

        let picker = self.kit_picker.as_mut().expect("checked above");
        if !picker.seeded {
            picker.seeded = true;
            if let Some(first) = shown.iter().find(|s| s.used) {
                picker.open.insert(first.id.clone());
            }
        }

        // Beside the toolbar, bottom-aligned with the button (3A); with the
        // toolbar hidden, at the canvas's bottom-left.
        let anchor = if self.kit_button.is_finite() && self.show_toolbar {
            egui::pos2(self.kit_button.right() + 8.0, self.kit_button.bottom())
        } else {
            self.canvas_rect.left_bottom() + egui::vec2(8.0, -8.0)
        };
        let height = panel_height(anchor.y, self.canvas_rect.top());
        let mut act: Option<Act> = None;
        let query = picker.query.trim().to_lowercase();
        let results: Vec<Item> = if query.is_empty() {
            Vec::new()
        } else {
            search(&query, &local, &shown)
        };
        // **Only while the search field holds the keyboard** (§15 D1023, the
        // kits QA review's finding): the field gives the keyboard back for good
        // (`D1014`'s fix (b)), and read from anywhere, `Enter` stepping into a
        // group, committing an inspector field or a dialog's note also placed the
        // highlighted result, and the arrows nudged the selection and moved the
        // highlight at once. Asked before the field is drawn, which is before a
        // single-line field lets go of the keyboard on `Enter`.
        let typing = ctx.memory(|m| m.has_focus(egui::Id::new(SEARCH_ID)));
        if typing && !results.is_empty() {
            let (down, up, enter) = ctx.input(|i| {
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    i.key_pressed(egui::Key::Enter),
                )
            });
            if down {
                picker.highlight = (picker.highlight + 1).min(results.len() - 1);
            }
            if up {
                picker.highlight = picker.highlight.saturating_sub(1);
            }
            picker.highlight = picker.highlight.min(results.len() - 1);
            if enter {
                act = Some(Act::Place(results[picker.highlight].source.clone()));
            }
        }

        // Placed by its top-left, not pivoted on its bottom: a pivot places the
        // area by *last* frame's size, and with the height fixed the top edge is
        // known before the frame starts.
        let panel = egui::Area::new(egui::Id::new("kit-picker"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor - egui::vec2(0.0, height))
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(color::CARD)
                    .corner_radius(RADIUS)
                    .stroke(egui::Stroke::new(1.0, color::text_a(20)))
                    .inner_margin(PAD)
                    .shadow(egui::Shadow {
                        offset: [0, 20],
                        blur: 50,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(128),
                    })
                    .show(ui, |ui| {
                        ui.set_width(W - 2.0 * PAD);
                        // The gaps are the constants `list_height` counts, so the
                        // panel comes out exactly `height` tall.
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if search_field(ui, &mut picker.query, &mut picker.focus) {
                            picker.highlight = 0;
                        }
                        ui.add_space(SEARCH_GAP);
                        // Not shrunk to its content in either axis: the list's
                        // height is the panel's, empty or full. ⚠️ **The minimum
                        // is load-bearing**: an `Area`'s `max_rect` is *last*
                        // frame's size, so a list asking only for room up to a
                        // maximum gets what the panel had before — which is how
                        // it stayed at its first height as kits opened. Only
                        // `min_scrolled_height` is taken whatever is available.
                        let list = list_height(height);
                        egui::ScrollArea::vertical()
                            .max_height(list)
                            .min_scrolled_height(list)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                if !query.is_empty() {
                                    results_list(ui, &results, picker.highlight, &mut act);
                                } else {
                                    browse(ui, &local, &shown, &picker.open, &mut act);
                                }
                            });
                    });
            })
            .response
            .rect;

        match act {
            Some(Act::Toggle(id)) => {
                if let Some(p) = self.kit_picker.as_mut()
                    && !p.open.remove(&id)
                {
                    p.open.insert(id);
                }
            }
            Some(Act::Place(source)) => self.place_from_picker(ctx, source),
            Some(Act::Drop(source, at)) => {
                if dropped_on_canvas(at, self.canvas_rect, panel) {
                    self.place_kit_at(ctx, source, at);
                }
            }
            Some(Act::OpenLibrary) => {
                self.kit_picker = None;
                self.go_to_dashboard();
                self.dash.nav = crate::panels::dashboard::Nav::Kits;
            }
            None => {}
        }
    }

    /// Every kit the picker lists: the ones this document uses — found in the
    /// library or not — by last use, then the library's others by name.
    fn shown_kits(&mut self, ctx: &egui::Context, library: &[KitEntry]) -> Vec<Shown> {
        let doc = &self.session.doc;
        let mut used: Vec<String> = Vec::new();
        if let Some(v) = doc.vault() {
            for c in doc
                .get(v)
                .map(|n| n.children().to_vec())
                .unwrap_or_default()
            {
                if let Some(o) = doc.get(c).and_then(|n| n.origin())
                    && !used.contains(&o.kit)
                {
                    used.push(o.kit.clone());
                }
            }
        }
        let order = doc
            .meta()
            .id
            .as_ref()
            .and_then(|d| self.library.local.kit_use.get(d))
            .cloned()
            .unwrap_or_default();
        used.sort_by_key(|k| order.iter().position(|o| o == k).unwrap_or(usize::MAX));

        let mut out = Vec::new();
        for id in &used {
            match library.iter().find(|k| &k.id == id) {
                Some(k) => out.push(self.shown_from_library(ctx, k, true)),
                None => out.push(self.shown_unavailable(id)),
            }
        }
        for k in library.iter().filter(|k| !used.contains(&k.id)) {
            out.push(self.shown_from_library(ctx, k, false));
        }
        out
    }

    /// A kit found in the library, its components once its published state is in.
    fn shown_from_library(&mut self, ctx: &egui::Context, k: &KitEntry, used: bool) -> Shown {
        let mut shown = Shown {
            id: k.id.clone(),
            name: k.name.clone(),
            status: format!("v{}", k.version),
            used,
            groups: Vec::new(),
            note: None,
        };
        match self.kit_shelf.load(&k.id) {
            Some(Load::Ready(Some(p))) => {
                let p = p.clone();
                // What a placement records: the version this state was read at,
                // which a publish since the scan may have moved past the scan's
                // (§15 D1023).
                let version = self.kit_shelf.read_version(&k.id).unwrap_or(k.version);
                shown.status = if used {
                    format!("v{version} · {}", p.tops().len())
                } else {
                    match p.tops().len() {
                        1 => "1 component".to_string(),
                        n => format!("{n} components"),
                    }
                };
                for top in p.tops().to_vec() {
                    let src = p.document();
                    let Some(n) = src.get(top) else { continue };
                    let item = Item {
                        source: Source::Kit {
                            id: k.id.clone(),
                            version,
                            published: p.clone(),
                            top,
                        },
                        name: n.name().to_string(),
                        set: n.set().is_some(),
                        kit: Some((k.name.clone(), used)),
                        tile: self.kit_shelf.tile(ctx, &k.id, top),
                        aliases: aliases(src, top),
                    };
                    let group = p.group(top).unwrap_or(&k.name).to_string();
                    push_grouped(&mut shown.groups, group, item);
                }
            }
            Some(Load::Ready(None)) => {
                shown.status = "not published".into();
                shown.note = Some(format!(
                    "Nothing published yet. Its author publishes from the top bar of {}.",
                    k.name
                ));
            }
            Some(Load::Failed) => {
                shown.status = "Unreadable".into();
                shown.note = Some("This kit's file could not be read.".into());
            }
            Some(Load::Pending) | None => {}
        }
        shown
    }

    /// A kit this document uses that the library does not have (3I) — its
    /// copies here still insert, because they are local.
    fn shown_unavailable(&self, id: &str) -> Shown {
        let doc = &self.session.doc;
        let copies: Vec<NodeId> = doc
            .vault()
            .and_then(|v| doc.get(v))
            .map(|v| v.children().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|c| {
                doc.get(*c)
                    .and_then(|n| n.origin())
                    .is_some_and(|o| o.kit == id)
            })
            .collect();
        let mut groups = Vec::new();
        for c in &copies {
            let Some(n) = doc.get(*c) else { continue };
            push_grouped(
                &mut groups,
                String::new(),
                Item {
                    source: Source::Local(*c),
                    name: n.name().to_string(),
                    set: n.set().is_some(),
                    kit: None,
                    tile: None,
                    aliases: aliases(doc, *c),
                },
            );
        }
        // The name the kit had when its copies were vendored — the library cannot
        // say, being exactly what lost it (`Origin::name`).
        let name = copies
            .iter()
            .find_map(|c| doc.get(*c)?.origin()?.name.clone())
            .unwrap_or_else(|| "Kit not found".into());
        Shown {
            id: id.to_string(),
            name,
            status: "Unavailable".into(),
            used: true,
            note: Some(format!(
                "Not found in your library folder. The {} component{} already in this \
                 document keep working, and you can still insert them.",
                copies.len(),
                if copies.len() == 1 { "" } else { "s" }
            )),
            groups,
        }
    }

    /// This document's own mains and sets, in document order — 3G's *This
    /// document*, and search's local results.
    fn local_items(&self) -> Vec<Item> {
        let doc = &self.session.doc;
        let mut out = Vec::new();
        let mut stack = vec![doc.root()];
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            if id != doc.root() && (n.set().is_some() || n.component()) {
                out.push(Item {
                    source: Source::Local(id),
                    name: n.name().to_string(),
                    set: n.set().is_some(),
                    kit: Some(("This document".into(), true)),
                    tile: None,
                    aliases: aliases(doc, id),
                });
                continue;
            }
            if n.link().is_none() {
                stack.extend(n.children().iter().rev().copied());
            }
        }
        out
    }

    /// Place an instance of `source` at the viewport's centre — a tile's click
    /// (3A).
    fn place_from_picker(&mut self, ctx: &egui::Context, source: Source) {
        self.place_kit_at(ctx, source, self.canvas_rect.center());
    }

    /// Place an instance of `source` centred on the screen point `at`, at the
    /// document's top level, as one undoable step, and select it — a click's
    /// viewport centre, or where a dragged tile was let go.
    fn place_kit_at(&mut self, ctx: &egui::Context, source: Source, at: egui::Pos2) {
        let root = self.session.doc.root();
        let built = match &source {
            Source::Kit {
                id,
                version,
                published,
                top,
            } => ondin_core::kit::insert(
                &self.session.doc,
                &mut self.session.ids,
                id,
                *version,
                published,
                *top,
                root,
            )
            .map_err(|e| match e {
                ondin_core::kit::KitError::NeedsUpdate(_) => {
                    "This kit has an update to accept before that component can be placed"
                        .to_string()
                }
                other => format!("That component could not be placed ({other:?})"),
            }),
            Source::Local(main) => {
                let main = match self.session.doc.get(*main) {
                    Some(n) if n.set().is_some() => {
                        ondin_core::kit::default_variant(&self.session.doc, *main)
                    }
                    Some(_) => Some(*main),
                    None => None,
                };
                main.and_then(|m| self.session.doc.capture_subtree(m))
                    .map(|nodes| {
                        let (tx, made) = ondin_core::build::insert_subtrees(
                            &self.session.doc,
                            &mut self.session.ids,
                            &[ondin_core::build::Placement {
                                nodes,
                                parent: root,
                                index: None,
                            }],
                            Vec2::ZERO,
                        );
                        (tx, made[0])
                    })
                    .ok_or_else(|| "That component is no longer here".to_string())
            }
        };
        let (mut tx, instance) = match built {
            Ok(b) => b,
            Err(why) => {
                self.session.fail(why);
                return;
            }
        };
        let centre = self.to_world(at, self.canvas_rect, ctx.pixels_per_point());
        if let Some(op) = centred(&self.session.doc, &tx, instance, centre) {
            tx.0.push(op);
        }
        if !self.session.commit(tx) {
            return;
        }
        self.session.selection.set_one(instance);
        if let Source::Kit { id, .. } = &source
            && let Some(doc) = self.session.doc.meta().id.clone()
        {
            let order = self.library.local.kit_use.entry(doc).or_default();
            order.retain(|k| k != id);
            order.insert(0, id.clone());
            self.library.local.save();
        }
    }
}

/// The panel's whole height with its foot at `bottom`: [`H`], or less when the
/// canvas above the button is shorter than that, never so little that the
/// search box and a row of tiles do not fit.
fn panel_height(bottom: f32, canvas_top: f32) -> f32 {
    let floor = 2.0 * PAD + SEARCH_H + SEARCH_GAP + TILE_H + 22.0;
    H.min(bottom - canvas_top - 8.0).max(floor)
}

/// Whether a tile let go at `at` lands on the canvas: inside it, and not back
/// on the panel it came from — a drag that ends where it started is a change
/// of mind, not a placement.
fn dropped_on_canvas(at: egui::Pos2, canvas: egui::Rect, panel: egui::Rect) -> bool {
    canvas.contains(at) && !panel.contains(at)
}

/// The scrolled list's height inside a panel `height` tall — less the padding,
/// the search box, and the frame's hairline at top and bottom.
fn list_height(height: f32) -> f32 {
    height - 2.0 * PAD - 2.0 - SEARCH_H - SEARCH_GAP
}

/// The `SetTransform` that puts `instance` — just inserted by `tx` at its main's
/// own transform — with its box centred on `centre`. Measured by laying out the
/// document the transaction leaves, which is the one place the copy's box is
/// known whatever the main is: a vendored main is never resolved, and a group's
/// box is its children's.
fn centred(
    doc: &Document,
    tx: &Transaction,
    instance: NodeId,
    centre: ondin_core::kurbo::Point,
) -> Option<Operation> {
    let mut scratch = doc.clone();
    scratch.apply(tx).ok()?;
    let node = scratch.get(instance)?;
    let at = node.transform();
    let bounds: Rect = match node.kind() {
        // A frame's box is its size, with no layout to wait for.
        NodeKind::Artboard { size } => {
            at.transform_rect_bbox(Rect::from_origin_size((0.0, 0.0), *size))
        }
        _ => ondin_core::Resolved::rebuild(&scratch).world_bounds(instance)?,
    };
    let shift = centre - bounds.center();
    Some(Operation::SetTransform {
        id: instance,
        transform: Affine::translate(shift) * at,
    })
}

/// Every name `top` is found by: its own, and for a set each variant's name and
/// value — *primary* finds *Button* (3F).
fn aliases(doc: &Document, top: NodeId) -> Vec<String> {
    let Some(n) = doc.get(top) else {
        return Vec::new();
    };
    let mut out = vec![n.name().to_lowercase()];
    if n.set().is_some() {
        for c in n.children() {
            if let Some(v) = doc.get(*c) {
                out.push(v.name().to_lowercase());
                out.extend(v.variant().iter().map(|s| s.to_lowercase()));
            }
        }
    }
    out
}

fn push_grouped(groups: &mut Vec<(String, Vec<Item>)>, group: String, item: Item) {
    match groups.iter_mut().find(|(g, _)| *g == group) {
        Some((_, items)) => items.push(item),
        None => groups.push((group, vec![item])),
    }
}

/// The search: every component of every kit, this document's own first among
/// equals, ranked — a name that starts with the query, then one that holds it,
/// then a match through a set's variants (3F).
fn search(query: &str, local: &[Item], shown: &[Shown]) -> Vec<Item> {
    let mut scored: Vec<(u8, Item)> = Vec::new();
    let items = local.iter().chain(
        shown
            .iter()
            .flat_map(|s| s.groups.iter().flat_map(|(_, i)| i)),
    );
    for item in items {
        let name = item.name.to_lowercase();
        let score = if name.starts_with(query) {
            0
        } else if name.contains(query) {
            1
        } else if item.aliases.iter().any(|a| a.contains(query)) {
            2
        } else {
            continue;
        };
        scored.push((score, item.clone()));
    }
    scored.sort_by_cached_key(|(score, item)| (*score, item.name.to_lowercase()));
    scored
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(_, i)| i)
        .collect()
}

/// The search field's id, which the result keys ask about.
const SEARCH_ID: &str = "kit-picker-search";

/// The search box: a magnifier, the field, and a clear button once it holds
/// something. Takes the keyboard when `focus` asks, once. Answers whether it
/// changed.
fn search_field(ui: &mut egui::Ui, query: &mut String, focus: &mut bool) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SEARCH_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    p.rect_filled(rect, egui::CornerRadius::same(6), color::text_a(12));
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(6),
        egui::Stroke::new(1.0, color::text_a(20)),
        egui::StrokeKind::Inside,
    );
    p.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        icon::MAGNIFYING_GLASS,
        theme::icon_font(14.0),
        theme::text::FAINT,
    );
    let field = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 30.0, rect.top()),
        egui::pos2(rect.right() - 28.0, rect.bottom()),
    );
    let edit = ui.put(
        field,
        egui::TextEdit::singleline(query)
            .id(egui::Id::new(SEARCH_ID))
            .frame(egui::Frame::NONE)
            .hint_text(
                egui::RichText::new("Search components")
                    .size(12.0)
                    .color(color::text_a(102)),
            )
            .font(egui::FontId::proportional(12.0))
            .vertical_align(egui::Align::Center),
    );
    // Focused once, when the picker opens — not whenever nothing else holds the
    // keyboard, which kept it focused and the keymap dead for as long as the
    // picker was open: a tool letter after a placement typed into the search
    // (`D1014`'s fix (b)).
    if *focus {
        *focus = false;
        edit.request_focus();
    }
    // ⚠️ **No `Escape` arm here.** egui clears focus on `Escape` in `begin_pass`
    // (§15 D317), so the press reaches the keymap and the escape ladder's rung
    // closes the picker — and an arm here, reading the field's `lost_focus`,
    // closed it a second time beside whatever the ladder had closed instead (an
    // open dropdown, the colour picker): `D1014`'s fix (a) again, through a
    // narrower door (`arch-scribe`'s reading).
    let mut changed = edit.changed();
    if !query.is_empty() {
        let x = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 14.0, rect.center().y),
            egui::vec2(20.0, 20.0),
        );
        let resp = ui.interact(x, egui::Id::new("kit-picker-clear"), egui::Sense::click());
        ui.painter().text(
            x.center(),
            egui::Align2::CENTER_CENTER,
            icon::X,
            theme::icon_font(12.0),
            if resp.hovered() {
                color::TEXT
            } else {
                theme::text::DIM
            },
        );
        if resp.on_hover_text("Clear").clicked() {
            query.clear();
            changed = true;
        }
    }
    changed
}

/// Browsing: this document's mains first when there are no kits (3G), then each
/// kit, used ones first, then the library's others under their heading (3E).
fn browse(
    ui: &mut egui::Ui,
    local: &[Item],
    shown: &[Shown],
    open: &HashSet<String>,
    act: &mut Option<Act>,
) {
    if shown.is_empty() {
        if !local.is_empty() {
            section_row(ui, "This document", &local.len().to_string());
            tiles(ui, local, act);
            ui.add_space(8.0);
        }
        if empty_note(ui) {
            *act = Some(Act::OpenLibrary);
        }
        return;
    }
    let mut heading_drawn = false;
    for s in shown {
        if !s.used && !heading_drawn {
            heading_drawn = true;
            ui.add_space(6.0);
            caps(ui, "In your library, not used here");
        }
        let is_open = open.contains(&s.id);
        if kit_header(ui, s, is_open).clicked() {
            *act = Some(Act::Toggle(s.id.clone()));
        }
        if !is_open {
            continue;
        }
        if let Some(note) = &s.note {
            ui.label(egui::RichText::new(note).size(11.0).color(theme::text::DIM));
        }
        if s.groups.is_empty() && s.note.is_none() {
            ui.label(
                egui::RichText::new("Loading…")
                    .size(11.0)
                    .color(theme::text::FAINT),
            );
        }
        for (group, items) in &s.groups {
            if !group.is_empty() {
                ui.label(
                    egui::RichText::new(group)
                        .size(10.5)
                        .color(color::text_a(107)),
                );
            }
            tiles(ui, items, act);
        }
        ui.add_space(4.0);
    }
}

/// A kit's header: the disclosure caret, the kit glyph, its name, and its status
/// on the right (3E).
fn kit_header(ui: &mut egui::Ui, s: &Shown, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, egui::CornerRadius::same(5), color::text_a(12));
    }
    let y = rect.center().y;
    let ink = if s.used {
        color::TEXT
    } else {
        color::text_a(191)
    };
    p.text(
        egui::pos2(rect.left() + 4.0, y),
        egui::Align2::LEFT_CENTER,
        if open {
            icon::CARET_DOWN
        } else {
            icon::CARET_RIGHT
        },
        theme::icon_font(11.0),
        theme::text::DIM,
    );
    p.text(
        egui::pos2(rect.left() + 20.0, y),
        egui::Align2::LEFT_CENTER,
        icon::PACKAGE,
        theme::icon_font(13.0),
        theme::text::MUTED,
    );
    p.text(
        egui::pos2(rect.left() + 38.0, y),
        egui::Align2::LEFT_CENTER,
        &s.name,
        egui::FontId::proportional(12.0),
        ink,
    );
    p.text(
        egui::pos2(rect.right() - 4.0, y),
        egui::Align2::RIGHT_CENTER,
        &s.status,
        egui::FontId::proportional(10.5),
        color::text_a(102),
    );
    resp
}

/// The tile grid: three to a row, each a picture and a name (3E).
fn tiles(ui: &mut egui::Ui, items: &[Item], act: &mut Option<Act>) {
    let w = ui.available_width();
    let tile_w = (w - GAP_X * (COLS as f32 - 1.0)) / COLS as f32;
    for row in items.chunks(COLS) {
        let (band, _) = ui.allocate_exact_size(egui::vec2(w, TILE_H + 22.0), egui::Sense::hover());
        for (i, item) in row.iter().enumerate() {
            let x = band.left() + i as f32 * (tile_w + GAP_X);
            let cell = egui::Rect::from_min_size(
                egui::pos2(x, band.top()),
                egui::vec2(tile_w, TILE_H + 22.0),
            );
            let resp = ui
                .interact(
                    cell,
                    // Keyed by what the tile places, not where it sits: a list
                    // scrolled under a drag would otherwise re-key the tile and
                    // drop the drag.
                    ui.id().with(match &item.source {
                        Source::Local(id) => ("kit-tile", String::new(), *id),
                        Source::Kit { id, top, .. } => ("kit-tile", id.clone(), *top),
                    }),
                    egui::Sense::click_and_drag(),
                )
                .on_hover_text(match &item.kit {
                    Some((kit, _)) => format!("{} · {kit}", item.name),
                    None => item.name.clone(),
                });
            draw_tile(ui, cell, item, resp.hovered() || resp.dragged());
            if resp.clicked() {
                *act = Some(Act::Place(item.source.clone()));
            }
            // A drag carries the tile under the pointer and places where it is
            // let go (3A) — on the canvas, which `kit_picker_ui` decides.
            if resp.dragged()
                && let Some(at) = ui.ctx().pointer_latest_pos()
            {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                let ghost = egui::Rect::from_center_size(at, cell.size());
                let layer =
                    egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("kit-tile-drag"));
                let painter = ui.ctx().layer_painter(layer);
                // On the panel's ground, so the translucent well reads over
                // the canvas as it does in the grid.
                painter.rect_filled(ghost.expand(4.0), egui::CornerRadius::same(7), color::CARD);
                draw_tile_on(&painter, ghost, item, true);
            }
            if resp.drag_stopped()
                && let Some(at) = ui.ctx().pointer_latest_pos()
            {
                *act = Some(Act::Drop(item.source.clone(), at));
            }
        }
        ui.add_space(GAP_Y - 4.0);
    }
}

fn draw_tile(ui: &egui::Ui, cell: egui::Rect, item: &Item, lit: bool) {
    draw_tile_on(ui.painter(), cell, item, lit);
}

/// A tile on any painter — the grid's, or the drag layer's under the pointer.
fn draw_tile_on(p: &egui::Painter, cell: egui::Rect, item: &Item, lit: bool) {
    let well = egui::Rect::from_min_size(cell.min, egui::vec2(cell.width(), TILE_H));
    p.rect_filled(well, egui::CornerRadius::same(5), color::text_a(26));
    if lit {
        p.rect_stroke(
            well,
            egui::CornerRadius::same(5),
            egui::Stroke::new(1.0, color::text_a(64)),
            egui::StrokeKind::Inside,
        );
    }
    if let Some(t) = &item.tile {
        let room = well.shrink(6.0);
        let size = t.size_vec2();
        let k = (room.width() / size.x).min(room.height() / size.y);
        let shown = egui::Rect::from_center_size(room.center(), size * k);
        p.image(t.id(), shown, crate::thumbs::FULL_UV, egui::Color32::WHITE);
    }
    let y = well.bottom() + 11.0;
    let gx = cell.left() + 6.0;
    if item.set {
        p.text(
            egui::pos2(gx, y),
            egui::Align2::CENTER_CENTER,
            icon::SQUARES_FOUR,
            theme::icon_font(11.0),
            color::text_a(160),
        );
    } else {
        crate::ui::paint_hexagon_filled(p, egui::pos2(gx, y), 10.0, color::text_a(160));
    }
    let text = p.layout(
        item.name.clone(),
        egui::FontId::proportional(11.0),
        color::text_a(204),
        cell.width() - 14.0,
    );
    let at = egui::pos2(cell.left() + 14.0, y - text.size().y / 2.0);
    p.with_clip_rect(cell).galley(at, text, color::text_a(204));
}

/// A search result: a small picture, the name, and the kit on the right (3F).
fn results_list(ui: &mut egui::Ui, items: &[Item], highlight: usize, act: &mut Option<Act>) {
    for (i, item) in items.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::click(),
        );
        let p = ui.painter();
        if i == highlight || resp.hovered() {
            p.rect_filled(rect, egui::CornerRadius::same(5), color::text_a(20));
        }
        let thumb = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 6.0, rect.center().y - 9.0),
            egui::vec2(28.0, 18.0),
        );
        p.rect_filled(thumb, egui::CornerRadius::same(3), color::text_a(26));
        if let Some(t) = &item.tile {
            let size = t.size_vec2();
            let k = (thumb.width() / size.x).min(thumb.height() / size.y);
            let shown = egui::Rect::from_center_size(thumb.center(), size * k);
            p.image(t.id(), shown, crate::thumbs::FULL_UV, egui::Color32::WHITE);
        }
        let gy = rect.center().y;
        if item.set {
            p.text(
                egui::pos2(rect.left() + 46.0, gy),
                egui::Align2::CENTER_CENTER,
                icon::SQUARES_FOUR,
                theme::icon_font(11.0),
                color::text_a(160),
            );
        } else {
            crate::ui::paint_hexagon_filled(
                p,
                egui::pos2(rect.left() + 46.0, gy),
                10.0,
                color::text_a(160),
            );
        }
        p.text(
            egui::pos2(rect.left() + 56.0, gy),
            egui::Align2::LEFT_CENTER,
            &item.name,
            egui::FontId::proportional(12.0),
            color::TEXT,
        );
        if let Some((kit, used)) = &item.kit {
            let right = if *used {
                kit.clone()
            } else {
                format!("{kit} · not used")
            };
            p.text(
                egui::pos2(rect.right() - 6.0, gy),
                egui::Align2::RIGHT_CENTER,
                right,
                egui::FontId::proportional(10.5),
                color::text_a(102),
            );
        }
        if resp.clicked() {
            *act = Some(Act::Place(item.source.clone()));
        }
    }
    if items.is_empty() {
        ui.label(
            egui::RichText::new("No components match")
                .size(11.0)
                .color(theme::text::FAINT),
        );
    }
}

/// A section's label and its count (3G's *This document · 2*).
fn section_row(ui: &mut egui::Ui, label: &str, count: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let y = rect.center().y;
    crate::ui::paint_hexagon_filled(
        p,
        egui::pos2(rect.left() + 8.0, y),
        11.0,
        theme::text::MUTED,
    );
    p.text(
        egui::pos2(rect.left() + 20.0, y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        color::TEXT,
    );
    p.text(
        egui::pos2(rect.right() - 4.0, y),
        egui::Align2::RIGHT_CENTER,
        count,
        egui::FontId::proportional(10.5),
        color::text_a(102),
    );
}

/// The small-caps heading over the kits this document does not use (3E).
fn caps(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(9.5)
            .extra_letter_spacing(0.95)
            .color(color::text_a(97)),
    );
}

/// No kit anywhere in the library (3G): how one comes to exist, and *Open
/// library* — the dashboard's *Kits* filter, where any file can be made one
/// from its card. Answers whether *Open library* was clicked.
fn empty_note(ui: &mut egui::Ui) -> bool {
    let mut open = false;
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        ui.label(theme::icon_text(icon::PACKAGE, 18.0, theme::text::DIM));
        ui.label(
            egui::RichText::new("No kits in your library")
                .size(12.0)
                .color(color::TEXT),
        );
        ui.label(
            egui::RichText::new(
                "Any document can become one: open it and choose Make this a kit \
                 from the document menu.",
            )
            .size(11.0)
            .color(theme::text::DIM),
        );
        ui.add_space(8.0);
        open =
            crate::settings::text_button(ui, "Open library", crate::ui::FieldButton::Off).clicked();
        ui.add_space(10.0);
    });
    open
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::library::state::Library;
    use ondin_core::kurbo::Size;
    use ondin_core::meta::KitMark;
    use ondin_core::variant::{VariantProp, VariantSet};
    use ondin_core::{IdSource, NodeKind};
    use std::path::PathBuf;

    pub(crate) fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-kits-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn frame(id: NodeId, parent: NodeId, index: usize, w: f64, name: &str) -> Operation {
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(w, 30.0),
            },
            transform: None,
            name: Some(name.into()),
        }
    }

    /// A kit *Atlas* filed in `root`, published at version 4: a set *Button* of
    /// *Small* and *Large* in a frame *Forms*, and a lone main *Badge*.
    pub(crate) fn file_kit(root: &std::path::Path) -> (String, NodeId, NodeId) {
        let mut ids = IdSource::new(0xAC);
        let r = ids.mint();
        let mut doc = Document::new(r);
        let [forms, set, small, large, badge] = [(); 5].map(|_| ids.mint());
        doc.apply(&Transaction(vec![
            frame(forms, r, 0, 400.0, "Forms"),
            frame(set, forms, 0, 300.0, "Button"),
            frame(small, set, 0, 80.0, "Small"),
            frame(large, set, 1, 120.0, "Large"),
            Operation::SetTransform {
                id: large,
                transform: Affine::translate((0.0, 60.0)),
            },
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
                        values: vec!["Small".into(), "Primary".into()],
                    }],
                }),
            },
            Operation::SetVariant {
                id: small,
                values: vec!["Small".into()],
            },
            Operation::SetVariant {
                id: large,
                values: vec!["Primary".into()],
            },
            frame(badge, r, 1, 24.0, "Badge"),
            Operation::SetComponent {
                id: badge,
                component: true,
            },
        ]))
        .unwrap();
        let path = crate::library::store::file_document(root, None, "Atlas", &mut doc).unwrap();
        let mut meta = doc.meta().clone();
        meta.kit = Some(KitMark {
            version: 4,
            ..KitMark::default()
        });
        doc.set_meta(meta);
        doc.set_published(ondin_core::kit::freeze(&doc));
        crate::library::store::write_document(&path, &doc).unwrap();
        (doc.meta().id.clone().unwrap(), set, badge)
    }

    /// A headless app on a library at `root`, editing a fresh filed document,
    /// with a canvas to aim at.
    pub(crate) fn app_on(ctx: &egui::Context, root: PathBuf) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = Library::open(root.clone());
        let mut doc = Document::new(app.session.ids.mint());
        let path = crate::library::store::file_document(&root, None, "Landing", &mut doc).unwrap();
        app.library.refresh();
        app.open_path(&path);
        app.canvas_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0));
        app
    }

    fn kit_source(app: &mut OndinApp, id: &str, top: NodeId) -> Source {
        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        let k = kits
            .iter()
            .find(|k| k.id == id)
            .expect("the kit is listed")
            .clone();
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let Some(Load::Ready(Some(p))) = app.kit_shelf.load(id) else {
            panic!("the kit's published state loads");
        };
        Source::Kit {
            id: k.id,
            version: k.version,
            published: p.clone(),
            top,
        }
    }

    /// **A kit's conflict copy loses, and a document is never its own kit**
    /// (§15 D1011 (10)): a sync client's `Atlas (conflicted copy …)` carries the
    /// same id, and the listing keeps the original whichever the scan met first.
    /// A kit that stopped sharing is not listed.
    #[test]
    fn the_library_lists_each_kit_once_and_not_the_document_itself() {
        let root = temp_root("listing");
        let (id, _, _) = file_kit(&root);
        let original = root.join("atlas.ondin");
        assert!(original.exists(), "the fixture's file name");
        std::fs::copy(
            &original,
            root.join("atlas (Jon's conflicted copy 2026-10-08).ondin"),
        )
        .unwrap();
        let mut lib = Library::open(root.clone());
        assert_eq!(lib.entries.len(), 2, "the fixture: both files scanned");
        // In both orders: the scan happens to meet the copy first (a space sorts
        // before a dot), so a listing where the last claimant won passed the
        // first half alone — flip-checked, and why the second half is here.
        for _ in 0..2 {
            let kits = kits_in(&lib, None);
            assert_eq!(kits.len(), 1, "{kits:?}");
            assert_eq!(kits[0].path, original, "the original wins");
            lib.entries.reverse();
        }
        let kits = kits_in(&lib, None);
        assert_eq!(kits[0].version, 4);
        assert!(
            kits_in(&lib, Some(&id)).is_empty(),
            "the kit itself lists no kit"
        );
        // **The original stops sharing; its conflict copy still says it is**
        // (§15 D1024): the kit is gone, in both orders — not stood in for by
        // the copy. Flip, run: sharing filtered before the tie-break fails
        // *"no kit once the original stops"*, the copy listed.
        for e in lib.entries.iter_mut().filter(|e| e.path == original) {
            e.meta.kit.as_mut().unwrap().sharing = false;
        }
        for _ in 0..2 {
            assert!(
                kits_in(&lib, None).is_empty(),
                "no kit once the original stops"
            );
            lib.entries.reverse();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The picker places a kit's component at the viewport's centre, in one
    /// undoable step** (§5.3e, 3A; §15 D1014): a set places its default variant —
    /// *Small*, first in reading order — vendored into the vault, the instance
    /// selected and centred, and the kit moved to the front of this document's
    /// last-use order. One undo takes all of it back, vault included.
    ///
    /// Flip, run: the centring `SetTransform` dropped fails the centre assertion,
    /// the instance left at its main's own place, `(40, 15)`.
    #[test]
    fn a_click_places_a_kit_component_at_the_viewport_centre() {
        let ctx = egui::Context::default();
        let root = temp_root("place");
        let (id, set, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let source = kit_source(&mut app, &id, set);
        app.place_from_picker(&ctx, source);

        let inst = app
            .session
            .selection
            .single()
            .expect("the instance selected");
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        assert!(app.session.doc.in_vault(main));
        assert_eq!(
            app.session.doc.get(main).unwrap().name(),
            "Small",
            "the default variant"
        );
        let centre = app.to_world(app.canvas_rect.center(), app.canvas_rect, 1.0);
        let at = app.session.preview_world_bounds(inst).unwrap().center();
        assert!((at - centre).hypot() < 1e-6, "{at:?} against {centre:?}");
        let doc_id = app.session.doc.meta().id.clone().unwrap();
        assert_eq!(app.library.local.kit_use[&doc_id], vec![id.clone()]);

        let source = kit_source(&mut app, &id, badge);
        app.place_from_picker(&ctx, source);
        assert_eq!(
            app.session
                .doc
                .get(app.session.doc.vault().unwrap())
                .unwrap()
                .children()
                .len(),
            2,
            "the set and the badge"
        );
        app.undo();
        app.undo();
        assert!(app.session.doc.vault().is_none(), "two undos, nothing left");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A vendored main names its kit, found or not** (4F, 3I): with the kit in
    /// the library the heading is its name and the copy's version; with the file
    /// gone it is the name the copy recorded when it was vendored
    /// (`Origin::name`), and *unavailable*. A main of the document's own has none.
    ///
    /// Flip, run: `vendor` writing no name onto the top fails *"named by the copy
    /// once the kit is gone"*, the heading falling back to *Kit*.
    #[test]
    fn a_vendored_main_names_its_kit_even_when_the_kit_is_gone() {
        let ctx = egui::Context::default();
        let root = temp_root("heading");
        let (id, _, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let source = kit_source(&mut app, &id, badge);
        app.place_from_picker(&ctx, source);
        let inst = app.session.selection.single().unwrap();
        let main = app.session.doc.get(inst).unwrap().link().unwrap();

        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        let found = super::super::index::kit_heading(&app.session.doc, &kits, main);
        let read =
            |h: Option<super::super::index::KitHeading>| h.map(|h| (h.kit, h.name, h.status));
        assert_eq!(read(found), Some((id.clone(), "Atlas".into(), "v4".into())));
        assert_eq!(
            super::super::index::kit_heading(&app.session.doc, &kits, inst),
            None,
            "an instance in the tree is no kit's"
        );

        std::fs::remove_file(root.join("atlas.ondin")).unwrap();
        app.library.refresh();
        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        assert_eq!(
            read(super::super::index::kit_heading(
                &app.session.doc,
                &kits,
                main
            )),
            Some((id.clone(), "Atlas".into(), "unavailable".into())),
            "named by the copy once the kit is gone"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The picker gives the keyboard back, and `Escape` closes one thing**
    /// (§15 D1014's fixes (a) and (b)). Its search field takes the keyboard when
    /// the picker opens and only then — it used to retake it on any frame nothing
    /// else held it, which left the keymap dead for as long as the picker was
    /// open. And `Escape` is a rung of `OndinApp::escape` and nothing else: the
    /// picker reads no `Escape` itself, **with its field focused or not** — egui
    /// lets go of the field before the keymap reads the key (§15 D317), so a
    /// field arm closed the picker a second time beside whatever the ladder had
    /// closed (an open dropdown).
    ///
    /// Flip, run: `search_field` requesting focus whenever none is held fails
    /// *"the keyboard is given back"*; the field's old `Escape` arm, which closed
    /// the picker on the field's `lost_focus`, fails *"not with the field
    /// focused"*.
    #[test]
    fn the_picker_gives_the_keyboard_back_and_escape_closes_one_thing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("focus");
        let mut app = app_on(&ctx, root.clone());
        app.toggle_kit_picker();
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let field = egui::Id::new("kit-picker-search");
        assert!(ctx.memory(|m| m.has_focus(field)), "focused on opening");

        // The field let go — a click elsewhere, a placement — and stays let go.
        ctx.memory_mut(|m| m.surrender_focus(field));
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        }
        assert!(
            ctx.memory(|m| m.focused().is_none()),
            "the keyboard is given back"
        );

        // `Escape` is the ladder's, not the picker's — with the field let go, and
        // with it focused.
        let escape = || {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            input
        };
        let _ = ctx.run_ui(escape(), |_| app.kit_picker_ui(&ctx));
        assert!(app.kit_picker.is_some(), "the picker does not read the key");
        ctx.memory_mut(|m| m.request_focus(field));
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        assert!(ctx.memory(|m| m.has_focus(field)), "the fixture: focused");
        let _ = ctx.run_ui(escape(), |_| app.kit_picker_ui(&ctx));
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        assert!(app.kit_picker.is_some(), "not with the field focused");
        app.escape(&ctx);
        assert!(app.kit_picker.is_none(), "the ladder closes it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Browsing lists a kit by what it is to this document** (3E): one not used
    /// here sits under *In your library, not used here* with its component count,
    /// closed; once something from it is placed it moves above that heading, open,
    /// with its version and count, its components under the frame they sat in.
    #[test]
    fn browsing_moves_a_kit_above_the_fold_once_it_is_used() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("browse");
        let (id, _, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let painted = |app: &mut OndinApp| -> Vec<String> {
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
            app.kit_shelf.settle();
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
            let out = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect()
        };
        app.toggle_kit_picker();
        let text = painted(&mut app);
        for want in ["IN YOUR LIBRARY, NOT USED HERE", "Atlas", "2 components"] {
            assert!(text.contains(&want.to_string()), "{want}: {text:?}");
        }
        assert!(!text.contains(&"Badge".to_string()), "closed: {text:?}");

        let source = kit_source(&mut app, &id, badge);
        app.place_from_picker(&ctx, source);
        app.kit_picker = None;
        app.toggle_kit_picker();
        let text = painted(&mut app);
        for want in ["Atlas", "v4 · 2", "Forms", "Button", "Badge"] {
            assert!(text.contains(&want.to_string()), "{want}: {text:?}");
        }
        assert!(
            !text.contains(&"IN YOUR LIBRARY, NOT USED HERE".to_string()),
            "{text:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A tile dragged onto the canvas places there, and 3G's *Open library*
    /// goes to the dashboard's *Kits*** (§5.3e, 3A, 3G; §15 D1018). With no kit
    /// in the library the document's own main is a tile under *This document*;
    /// pressed, carried out of the panel and let go over the canvas, it places an
    /// instance centred on the point of release — driven by real pointer events.
    /// Let go back over the panel, nothing is placed.
    ///
    /// Flip, run: `dropped_on_canvas` answering `canvas.contains(at)` alone fails
    /// *"let go over the panel, nothing placed"*.
    #[test]
    fn a_dragged_tile_places_where_it_lands_and_open_library_goes_to_kits() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("drag");
        let mut app = app_on(&ctx, root.clone());
        let main = app.session.ids.mint();
        let doc_root = app.session.doc.root();
        assert!(app.session.commit(Transaction(vec![
            frame(main, doc_root, 0, 80.0, "Button"),
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ])));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let frame_with = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |_| app.kit_picker_ui(&ctx))
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.text().to_string(), t.pos + t.galley.size() / 2.0))
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let find = |text: &[(String, egui::Pos2)], want: &str| {
            text.iter()
                .find(|(t, _)| t == want)
                .map(|(_, at)| *at)
                .unwrap_or_else(|| panic!("{want}: {text:?}"))
        };
        let button = egui::PointerButton::Primary;
        let press = |at, pressed| egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: Default::default(),
        };
        let drag = |app: &mut OndinApp, from: egui::Pos2, to: egui::Pos2| {
            frame_with(app, vec![egui::Event::PointerMoved(from)]);
            frame_with(app, vec![press(from, true)]);
            for k in 1..=4 {
                let at = from + (to - from) * (k as f32 / 4.0);
                frame_with(app, vec![egui::Event::PointerMoved(at)]);
            }
            frame_with(app, vec![press(to, false)]);
        };

        app.toggle_kit_picker();
        frame_with(&mut app, vec![]);
        let text = frame_with(&mut app, vec![]);
        // The name sits under the tile's well; aim at the well.
        let tile = find(&text, "Button") - egui::vec2(0.0, 30.0);

        // Back over the panel: a change of mind.
        drag(&mut app, tile, tile + egui::vec2(60.0, 0.0));
        assert!(
            app.session.selection.single().is_none(),
            "let go over the panel, nothing placed"
        );

        let to = egui::pos2(600.0, 300.0);
        drag(&mut app, tile, to);
        let inst = app
            .session
            .selection
            .single()
            .expect("dropped on the canvas");
        assert_eq!(app.session.doc.get(inst).unwrap().link(), Some(main));
        let want = app.to_world(to, app.canvas_rect, 1.0);
        let at = app.session.preview_world_bounds(inst).unwrap().center();
        assert!((at - want).hypot() < 1e-6, "{at:?} against {want:?}");

        let text = frame_with(&mut app, vec![]);
        let open = find(&text, "Open library");
        frame_with(&mut app, vec![egui::Event::PointerMoved(open)]);
        frame_with(&mut app, vec![press(open, true)]);
        frame_with(&mut app, vec![press(open, false)]);
        assert!(app.kit_picker.is_none(), "the picker closes");
        assert!(app.view == crate::app::View::Dashboard);
        assert_eq!(app.dash.nav, crate::panels::dashboard::Nav::Kits);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The picker keeps one height whatever it holds**: a closed kit, the same
    /// kit open, and a search matching nothing all draw the panel at the same
    /// rect, its foot on the button. Reported as the panel keeping its first
    /// height as kits opened, hard to navigate: an `Area` lays its content out
    /// in last frame's rect, so the list never had room to grow into.
    ///
    /// Flip, run, both on `kit_picker_ui`'s list: `auto_shrink` back to
    /// `[false, true]` fails *"a closed kit"*, the panel 114 tall; dropping
    /// `min_scrolled_height` fails it too, the panel 400 tall — egui's default
    /// area height on the first frame, kept ever after.
    #[test]
    fn the_picker_keeps_one_height_whatever_it_holds() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("height");
        let (id, _, _) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        app.kit_button = egui::Rect::from_min_size(egui::pos2(4.0, 560.0), egui::vec2(32.0, 32.0));
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let rect = |app: &mut OndinApp| {
            for _ in 0..3 {
                let _ = ctx.run_ui(input(), |_| app.kit_picker_ui(&ctx));
                app.kit_shelf.settle();
            }
            let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("kit-picker"));
            ctx.memory(|m| m.area_rect(layer.id)).expect("drawn")
        };
        app.toggle_kit_picker();
        // `W` is the frame's inside; its hairline adds a point at each side.
        let want = egui::Rect::from_min_max(
            egui::pos2(44.0, 592.0 - H),
            egui::pos2(44.0 + W + 2.0, 592.0),
        );
        let closed = rect(&mut app);
        assert!(
            (closed.min - want.min).length() < 0.5 && (closed.max - want.max).length() < 0.5,
            "a closed kit: {closed:?} against {want:?}"
        );
        app.kit_picker.as_mut().unwrap().open.insert(id);
        assert_eq!(rect(&mut app), closed, "the kit open");
        app.kit_picker.as_mut().unwrap().query = "zzz".into();
        assert_eq!(rect(&mut app), closed, "a search matching nothing");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Search runs through a set's variants and `Enter` places the highlighted
    /// result** (3F): *primary* is one of Button's values and no component's name,
    /// so it finds the set; the picker is driven for real, two frames, then
    /// `Enter`.
    ///
    /// Flips, run: a set's variant values left out of its aliases fails the first
    /// assertion, the list reading *"No components match"*; the keys read
    /// whoever holds the keyboard (`typing` always true) fails *"not the
    /// picker's Enter"* (§15 D1023).
    #[test]
    fn search_finds_a_set_by_its_variant_and_enter_places_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("search");
        let (id, _, _) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        app.toggle_kit_picker();
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        app.kit_shelf.settle();
        app.kit_picker.as_mut().unwrap().query = "primary".into();
        let out = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let _ = out;
        let out = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let painted: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
        assert!(painted.contains(&"Button".to_string()), "{painted:?}");
        assert!(!painted.contains(&"Badge".to_string()), "{painted:?}");

        let enter = || {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            input
        };
        // **With the field let go, `Enter` is someone else's** (§15 D1023): a
        // group stepped into, a field committed — and nothing is placed.
        let field = egui::Id::new(SEARCH_ID);
        ctx.memory_mut(|m| m.surrender_focus(field));
        let before = app.session.doc.clone();
        let _ = ctx.run_ui(enter(), |_| app.kit_picker_ui(&ctx));
        assert_eq!(app.session.doc, before, "not the picker's Enter");

        ctx.memory_mut(|m| m.request_focus(field));
        let _ = ctx.run_ui(enter(), |_| app.kit_picker_ui(&ctx));
        let inst = app.session.selection.single().expect("Enter placed it");
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        assert_eq!(app.session.doc.get(main).unwrap().origin().unwrap().kit, id);
        let _ = std::fs::remove_dir_all(&root);
    }
}
