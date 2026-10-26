//! The floating insert picker (§5.3e, 3A–3I; §15 D1014).
//!
//! A panel beside the vertical toolbar, bottom-aligned with the hexagon at its
//! foot — `Shift+I` or the button opens it, `Escape` or the button closes it, and
//! nothing else closes or moves (3A). Browsing lists the kits this document uses
//! first, by last use, each open to its sets and lone mains grouped by the
//! top-level frame they sat in; then the library's other kits (3E). Typing turns
//! it into one flat list across every kit, best match first (3F). A click places
//! an instance at the viewport's centre, at the document's top level; a tile dragged
//! out of the panel and let go over the canvas places it centred there, inside the
//! frame or layout group under the pointer if there is one.

use super::index::{KitEntry, Load, Pixels, kits_in, raster_fit, tile_subject, tiles_of};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::kit::Published;
use ondin_core::kurbo::{Affine, Rect, Vec2};
use ondin_core::{Document, NodeId, Operation, Transaction};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, mpsc};

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
    /// The document's own mains' tile pictures (§15 D1031).
    local: LocalTiles,
    /// The dragged component's picture at canvas size (§15 D1029).
    ghost: Option<Ghost>,
    /// The panel's rect as last drawn: over it, a drag shows the tile.
    panel: Option<egui::Rect>,
}

impl Picker {
    /// Whether a component is being carried out of the panel — the right-click
    /// cancel's question (§15 D1033).
    pub(crate) fn dragging(&self) -> bool {
        self.ghost.is_some()
    }

    /// Drop the carried component's picture; the app's `gesture_cancelled`
    /// keeps the release from placing it (§15 D1033).
    pub(crate) fn cancel_drag(&mut self) {
        self.ghost = None;
    }

    /// A family the document names has arrived: every picture of its own mains
    /// was laid out without it, and nothing in the document changed to say so
    /// (§15 D1035).
    pub(crate) fn fonts_changed(&mut self) {
        self.local.invalidate();
    }
}

/// What names a placeable thing across frames: its kit (empty for this
/// document) and its top.
type Key = (String, NodeId);

fn key_of(source: &Source) -> Key {
    match source {
        Source::Local(id) => (String::new(), *id),
        Source::Kit { id, top, .. } => (id.clone(), *top),
    }
}

/// A placement asked for and not yet run (§15 D1030) — held on the app, not on
/// [`Picker`], so `Escape` closing the picker in the frames it waits does not
/// lose a click already made (`arch-scribe`'s reading of the first cut).
pub(crate) struct Placing {
    key: Key,
    source: Source,
    /// Where it lands: a drop's point, or `None` for the viewport's centre.
    at: Option<egui::Pos2>,
    /// Whether a frame has been drawn with the tile dimmed since it was asked.
    painted: bool,
}

/// The dragged component rendered at the canvas's zoom, off the frame.
struct Ghost {
    key: Key,
    /// The worker's answer: the picture and the component's size in world units.
    rx: Option<mpsc::Receiver<Option<(Pixels, egui::Vec2)>>>,
    picture: Option<(egui::TextureHandle, egui::Vec2)>,
    /// The canvas zoom the picture is (or is being) rendered at.
    zoom: f64,
}

/// The tile pictures of this document's own mains, rendered on a worker.
///
/// **Only the mains whose own subtree changed are rendered again, once the
/// edits have settled** (§15 D1034). The first cut rendered every main on every
/// revision with the picker open — a whole-document clone on the frame and a
/// layout and raster per main on the worker, for each nudge of an unrelated
/// layer. Now a new revision waits [`SETTLE`] for the next, then each main's
/// subtree is compared with the one its picture was drawn from, and the
/// document is cloned only when one differs. A nested instance's copies are
/// written into its main's subtree at the commit, so a change to the inner
/// main shows in the outer's comparison too.
#[derive(Default)]
struct LocalTiles {
    /// The revision last compared against.
    rev: Option<u64>,
    /// A revision seen and not yet compared, and when it was first seen.
    pending: Option<(u64, std::time::Instant)>,
    /// Each main's [`content_of`] — its subtree, the root's translation zeroed —
    /// as its picture was drawn from it.
    drawn: HashMap<NodeId, Vec<ondin_core::Node>>,
    /// The render in flight, and the mains it was asked for.
    job: Option<(TileJob, Vec<NodeId>)>,
    tiles: HashMap<NodeId, egui::TextureHandle>,
}

/// What a main's picture is drawn from, as [`LocalTiles`] compares it: its
/// subtree, **its own translation taken out** (§15 D1035). A main moved on the
/// canvas draws the same picture, so it is not drawn again; one rotated or
/// scaled does not, and its transform's other terms stay in the comparison.
fn content_of(doc: &Document, top: NodeId) -> Vec<ondin_core::Node> {
    let mut nodes = doc.capture_subtree(top).unwrap_or_default();
    if let Some(root) = nodes.first_mut() {
        let [a, b, c, d, _, _] = root.transform().as_coeffs();
        *root = root.with_transform(Affine::new([a, b, c, d, 0.0, 0.0]));
    }
    nodes
}

/// A local render's answer: each main's picture.
type TileJob = mpsc::Receiver<HashMap<NodeId, Pixels>>;

/// How long the document has to stand still before its mains are compared.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

impl LocalTiles {
    /// Take a finished job; once the document has settled on a new revision,
    /// compare the mains and render the ones that changed. Old pictures stand
    /// until new ones land; a main gone takes its picture with it.
    fn pass(&mut self, ctx: &egui::Context, doc: &Document, rev: u64, tops: &[NodeId]) {
        if let Some((rx, _)) = &self.job {
            match rx.try_recv() {
                Ok(pixels) => {
                    self.job = None;
                    self.land(ctx, pixels);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if let Some((_, asked)) = self.job.take() {
                        self.forget(&asked);
                    }
                }
            }
        }
        if self.rev == Some(rev) {
            return;
        }
        // The first look at a document is not waited for: there is nothing to
        // show until it is drawn.
        if self.rev.is_some() {
            match self.pending {
                Some((r, since)) if r == rev => {
                    // One clock read: testing `elapsed() < SETTLE` and then
                    // subtracting a second `elapsed()` underflowed a `Duration`
                    // — a panic in every profile — on a frame that landed a few
                    // nanoseconds before the deadline, the very frame the
                    // `request_repaint_after` below asks for (`[X4.1-L1-01]` of
                    // the v0.5.0 release review; the crate denies the lint).
                    let left = SETTLE.saturating_sub(since.elapsed());
                    if !left.is_zero() {
                        ctx.request_repaint_after(left);
                        return;
                    }
                }
                _ => {
                    self.pending = Some((rev, std::time::Instant::now()));
                    ctx.request_repaint_after(SETTLE);
                    return;
                }
            }
        }
        self.rev = Some(rev);
        self.pending = None;
        self.tiles.retain(|id, _| tops.contains(id));
        self.drawn.retain(|id, _| tops.contains(id));
        let mut changed = Vec::new();
        for top in tops {
            let now = content_of(doc, *top);
            if self.drawn.get(top) != Some(&now) {
                self.drawn.insert(*top, now);
                changed.push(*top);
            }
        }
        if changed.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let doc = doc.clone();
        let asked = changed.clone();
        let spawned = std::thread::Builder::new()
            .name("ondin-local-tiles".into())
            .spawn(move || {
                let _ = tx.send(tiles_of(&doc, &asked));
            });
        if spawned.is_ok() {
            self.job = Some((rx, changed));
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else {
            // Not drawn after all: compared afresh on the next revision rather
            // than counted as drawn (`arch-scribe`'s reading).
            self.forget(&changed);
        }
    }

    /// Draw every main again, now: forgotten, and compared on the next pass
    /// without the settle wait, as the first look is (§15 D1035).
    fn invalidate(&mut self) {
        self.drawn.clear();
        self.rev = None;
        self.pending = None;
    }

    /// Mains whose render never landed: not drawn, so the next revision's
    /// comparison asks for them again.
    fn forget(&mut self, mains: &[NodeId]) {
        for m in mains {
            self.drawn.remove(m);
        }
    }

    fn land(&mut self, ctx: &egui::Context, pixels: HashMap<NodeId, Pixels>) {
        for (id, p) in pixels {
            let t = p.texture(ctx, format!("local-tile-{}", id.to_wire()));
            self.tiles.insert(id, t);
        }
    }

    /// Wait for the job in flight — the tests'.
    #[cfg(test)]
    fn settle(&mut self, ctx: &egui::Context) {
        if let Some((rx, _)) = self.job.take()
            && let Ok(pixels) = rx.recv_timeout(std::time::Duration::from_secs(20))
        {
            self.land(ctx, pixels);
        }
    }

    /// Whether a render is in flight — the tests'.
    #[cfg(test)]
    fn busy(&self) -> bool {
        self.job.is_some()
    }
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
                // The variables browser shares the place (§5.3f, G11).
                self.var_browser = None;
                // A rescan reads each file's first 4 KB, so a kit published since
                // the last one — in this window or synced in — is seen at once
                // rather than when the dashboard next looks (`kits::index`).
                self.library.refresh();
                // And the shelf, against it: kits gone from the library let
                // go, and a read that failed is tried again (`KitShelf::refresh`).
                let listed =
                    super::index::kits_in(&self.library, self.session.doc.meta().id.as_deref());
                self.kit_shelf.refresh(&listed);
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
        // A placement asked for on an earlier frame, once a frame has shown its
        // tile dimmed (§15 D1030): the placement runs on this thread and the
        // window does not repaint while it does, so the dim has to be on screen
        // before the work starts or it is never seen. **Before the closed
        // picker's return**: a picker closed in the wait has no tile to dim, and
        // the click it took still places.
        let closed = self.kit_picker.is_none();
        if self
            .kit_placing
            .as_ref()
            .is_some_and(|pl| pl.painted || closed)
            && let Some(pl) = self.kit_placing.take()
        {
            let at = pl.at.unwrap_or(self.canvas_rect.center());
            self.place_kit_at(ctx, pl.source, at, pl.at.is_some());
        }
        if closed {
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
        let mut local = self.local_items();
        let tops: Vec<NodeId> = local
            .iter()
            .filter_map(|i| match i.source {
                Source::Local(id) => Some(id),
                Source::Kit { .. } => None,
            })
            .collect();
        let rev = self.session.revision();
        let zoom = self.session.camera.zoom;

        let picker = self.kit_picker.as_mut().expect("checked above");
        picker.local.pass(ctx, &self.session.doc, rev, &tops);
        for item in &mut local {
            if let Source::Local(id) = item.source {
                item.tile = picker.local.tiles.get(&id).cloned();
            }
        }
        let busy = self.kit_placing.as_ref().map(|p| p.key.clone());
        // Set by `cancel_gesture` on the right-click's press, cleared once no
        // button is down (§15 D1033).
        let cancelled = self.gesture_cancelled;
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
        // The item a tile or a result is being dragged from this frame.
        let mut dragging: Option<Item> = None;
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
                        let mut out = Out {
                            act: &mut act,
                            dragging: &mut dragging,
                            busy: busy.as_ref(),
                            cancelled,
                        };
                        egui::ScrollArea::vertical()
                            .max_height(list)
                            .min_scrolled_height(list)
                            .auto_shrink([false, false])
                            // No bar, as on the app's other panels (§15 D1028):
                            // the wheel and a drag still scroll it.
                            .scroll_bar_visibility(
                                egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                            )
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 4.0;
                                if !query.is_empty() {
                                    results_list(ui, &results, picker.highlight, &mut out);
                                } else {
                                    browse(ui, &local, &shown, &picker.open, &mut out);
                                }
                            });
                    });
            })
            .response
            .rect;

        let ppp = ctx.pixels_per_point();
        let canvas = self.canvas_rect;
        // The frame just drawn showed the waiting tile dimmed; the next one
        // places it.
        if let Some(pl) = self.kit_placing.as_mut() {
            pl.painted = true;
            ctx.request_repaint();
        }
        if let Some(p) = self.kit_picker.as_mut() {
            p.panel = Some(panel);
            match &dragging {
                Some(item) => {
                    let ghost = p
                        .ghost
                        .get_or_insert_with(|| Ghost::start(item, &self.session.doc, zoom));
                    if ghost.key != key_of(&item.source) {
                        *ghost = Ghost::start(item, &self.session.doc, zoom);
                    }
                    ghost.pass(ctx);
                    ghost.follow_zoom(item, &self.session.doc, zoom);
                    draw_ghost(ctx, item, ghost, panel, canvas, zoom, ppp);
                }
                None => p.ghost = None,
            }
        }

        match act {
            Some(Act::Toggle(id)) => {
                if let Some(p) = self.kit_picker.as_mut()
                    && !p.open.remove(&id)
                {
                    p.open.insert(id);
                }
            }
            Some(Act::Place(source)) => self.ask_place(source, None),
            Some(Act::Drop(source, at)) => {
                if dropped_on_canvas(at, self.canvas_rect, panel, ctx.layer_id_at(at)) {
                    self.ask_place(source, Some(at));
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
    ///
    /// **Used means an instance of it is on the page** (§15 D1028), not that its
    /// copies are in the vault: deleting a kit's last instance leaves the
    /// vendored main behind (it is what an undo or a paste needs), and the kit
    /// went on reading as used — no *"In your library, not used here"* — however
    /// long the document had none of it. A kit whose copies are vendored but
    /// whose file the library cannot find is still listed, by its copies (3I).
    fn shown_kits(&mut self, ctx: &egui::Context, library: &[KitEntry]) -> Vec<Shown> {
        let doc = &self.session.doc;
        let mut vaulted: Vec<String> = Vec::new();
        if let Some(v) = doc.vault() {
            for c in doc
                .get(v)
                .map(|n| n.children().to_vec())
                .unwrap_or_default()
            {
                if let Some(o) = doc.get(c).and_then(|n| n.origin())
                    && !vaulted.contains(&o.kit)
                {
                    vaulted.push(o.kit.clone());
                }
            }
        }
        let live = kits_on_the_page(doc);
        let order = doc
            .meta()
            .id
            .as_ref()
            .and_then(|d| self.library.local.kit_use.get(d))
            .cloned()
            .unwrap_or_default();
        let by_use = |k: &String| order.iter().position(|o| o == k).unwrap_or(usize::MAX);
        let mut used: Vec<String> = live.iter().cloned().collect();
        used.sort_by_key(by_use);
        // Vendored, not on the page, and not in the library: listed after the
        // used ones, unused, so its copies can still be inserted (3I).
        let mut lost: Vec<String> = vaulted
            .into_iter()
            .filter(|k| !live.contains(k) && !library.iter().any(|e| &e.id == k))
            .collect();
        lost.sort_by_key(by_use);

        let mut out = Vec::new();
        for id in &used {
            match library.iter().find(|k| &k.id == id) {
                Some(k) => out.push(self.shown_from_library(ctx, k, true)),
                None => out.push(self.shown_unavailable(id, true)),
            }
        }
        for id in &lost {
            out.push(self.shown_unavailable(id, false));
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
    fn shown_unavailable(&self, id: &str, used: bool) -> Shown {
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
            used,
            note: Some(format!(
                "Not found in your library folder. The {} component{} already in this \
                 document keep working, and you can still insert them.",
                copies.len(),
                if copies.len() == 1 { "" } else { "s" }
            )),
            groups,
        }
    }

    /// This document's own mains and sets, in document order — the *This
    /// document* section that leads the picker (§15 D1031, 3G's before it), and
    /// search's local results.
    fn local_items(&self) -> Vec<Item> {
        let doc = &self.session.doc;
        let mut out = Vec::new();
        // Every page's, in page order: a main is the document's, wherever it sits.
        let mut stack: Vec<NodeId> = doc.page_ids().collect();
        stack.reverse();
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            if !doc.is_page(id) && (n.set().is_some() || n.component()) {
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
    ///
    /// The tests' door: the picker's own clicks go through `Self::ask_place`,
    /// which waits a frame for the dim (§15 D1030). Plain backticks: a
    /// `cfg(test)` item's doc is one `cargo doc` never builds (§15 D319).
    #[cfg(test)]
    fn place_from_picker(&mut self, ctx: &egui::Context, source: Source) {
        self.place_kit_at(ctx, source, self.canvas_rect.center(), false);
    }

    /// The container a tile let go at `world` lands in: the topmost unlocked frame
    /// or group with a layout that shows under the point (its ancestors' eyes and
    /// clips included), `None` for the canvas. One that
    /// is or sits in a component is passed over, since the component rules may refuse
    /// an instance there, and so is one inside a boolean.
    fn drop_container_at(&self, world: ondin_core::kurbo::Point) -> Option<NodeId> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        // A boolean's operands are geometry, not places to put things.
        let refused = |f: NodeId| {
            std::iter::successors(doc.get(f), |n| n.parent().and_then(|p| doc.get(p))).any(|n| {
                n.component()
                    || n.link().is_some()
                    || n.set().is_some()
                    || matches!(n.kind(), ondin_core::NodeKind::Boolean { .. })
            })
        };
        let takes = |n: &ondin_core::Node| match n.kind() {
            ondin_core::NodeKind::Artboard { .. } => true,
            ondin_core::NodeKind::Group => n.display().is_some(),
            _ => false,
        };
        // Pre-order in child order is paint order: the last hit is the topmost.
        // A hidden layer's subtree is not descended, nor a clipping one's when
        // the point is outside it: nothing in there shows at the point.
        let mut hit = None;
        let page = self.session.page;
        let mut stack = vec![page];
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get(id) else { continue };
            let under = res.world_bounds(id).is_some_and(|b| b.contains(world));
            if id != page && !self.shown_visible(id) {
                continue;
            }
            if id != page
                && takes(node)
                && under
                && !ondin_core::is_effectively_locked(doc, id)
                && !refused(id)
            {
                hit = Some(id);
            }
            if id == page || under || !node.clip() {
                stack.extend(node.children().iter().rev().copied());
            }
        }
        hit
    }

    /// A click's, a drop's or `Enter`'s placement, run once a frame has shown
    /// its tile dimmed (§15 D1030) — the "heard you" the dashboard's card pulse
    /// is (§15 D1027), held at its low because the placement runs on this
    /// thread and nothing repaints while it does. One at a time: a second ask
    /// while one waits is dropped, as a second click on a busy button is.
    fn ask_place(&mut self, source: Source, at: Option<egui::Pos2>) {
        if self.kit_placing.is_none() {
            self.kit_placing = Some(Placing {
                key: key_of(&source),
                source,
                at,
                painted: false,
            });
        }
    }

    /// Place an instance of `source` centred on the screen point `at`, as one
    /// undoable step, and select it — a click's viewport centre at the document's top
    /// level, or where a dragged tile was let go (`dropped`), inside the container
    /// there ([`Self::drop_container_at`]).
    fn place_kit_at(&mut self, ctx: &egui::Context, source: Source, at: egui::Pos2, dropped: bool) {
        let centre = self.to_world(at, self.canvas_rect, ctx.pixels_per_point());
        let parent = dropped
            .then(|| self.drop_container_at(centre))
            .flatten()
            .unwrap_or(self.session.page);
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
                parent,
            )
            .map_err(|e| match e {
                // A main, or the kit's modes, held at an older version (§15
                // D1064): either way an update is owed first. A kit held newer,
                // covering the modes this kit read, is reused, not refused.
                ondin_core::kit::KitError::NeedsUpdate(_) | ondin_core::kit::KitError::Modes(_) => {
                    "This kit has an update to accept before that component can be placed"
                        .to_string()
                }
                // §15 D1036 (2): a kit built on this one.
                ondin_core::kit::KitError::Circular(own) => {
                    let src = published.document();
                    format!(
                        "{} in {} uses this kit's {}, so it cannot be placed here",
                        src.get(*top).map_or("That component", |n| n.name()),
                        published.name().unwrap_or("its kit"),
                        self.session.doc.get(own).map_or("component", |n| n.name()),
                    )
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
                                parent,
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
        if let Some(op) = centred(&self.session.doc, &tx, instance, centre) {
            tx.0.push(op);
        }
        self.session.name_step("Insert component", icon::HEXAGON);
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
///
/// **Nor over anything floating on the canvas** — `under` is the area there
/// (`Context::layer_id_at`), and only the canvas's own background layer, or
/// none, takes the drop. The inspector and the colour picker float over the
/// canvas's rect, and a tile let go on one was placed out of sight beneath it
/// (the v0.5.0 release review's `[X4.1-L1-03]`); aiming at a panel is the same
/// change of mind as aiming back at the picker.
///
/// ⚠️ **The floating status bar is not one of them, on purpose**: it is not
/// interactable, so every click passes through it to the canvas, and
/// `layer_id_at` skips it — a drop there lands as a click there would. This
/// comment named it among the panels until the record's reading of `bdabb66`
/// found egui skipping it; the bar takes nothing from the pointer, so the
/// comment was overclaiming, not the code.
fn dropped_on_canvas(
    at: egui::Pos2,
    canvas: egui::Rect,
    panel: egui::Rect,
    under: Option<egui::LayerId>,
) -> bool {
    canvas.contains(at)
        && !panel.contains(at)
        && under.is_none_or(|l| l.order == egui::Order::Background)
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
    let at = scratch.get(instance)?.transform();
    // The laid-out box for every kind, a frame's too: a frame that hugs its
    // content stores the size it was drawn at, not the one layout gives it
    // (§15 D874), and centring the stored box put a hugging main 55 px off the
    // aim (the v0.5.0 release review's `[R1-L2-01]`).
    let res = ondin_core::Resolved::rebuild(&scratch);
    let bounds: Rect = res.world_bounds(instance)?;
    // The shift is in world space; the transform is in the parent's, a frame's when
    // a tile was dropped on one.
    let parent = scratch.get(instance)?.parent()?;
    let into = res.world_transform(parent)?.inverse();
    let shift = into * centre - into * bounds.center();
    Some(Operation::SetTransform {
        id: instance,
        transform: Affine::translate(shift) * at,
    })
}

/// The kits an instance in the document comes from (§15 D1028): every node on a
/// page whose link — or nested swap — names a vendored main, by the kit its
/// vault copy records. The vault is a root of its own, so the walk never enters
/// it and a vendored main's own nested copies are not counted.
fn kits_on_the_page(doc: &Document) -> HashSet<String> {
    let mut out = HashSet::new();
    let Some(vault) = doc.vault() else {
        return out;
    };
    let kit_of = |main: NodeId| -> Option<String> {
        let mut top = main;
        for _ in 0..=doc.len() {
            let parent = doc.get(top)?.parent()?;
            if parent == vault {
                return doc.get(top)?.origin().map(|o| o.kit.clone());
            }
            top = parent;
        }
        None
    };
    let mut stack: Vec<NodeId> = doc.page_ids().collect();
    let mut seen = 0usize;
    while let Some(id) = stack.pop() {
        seen += 1;
        if seen > doc.len() + 1 {
            break;
        }
        let Some(n) = doc.get(id) else { continue };
        for main in [n.link(), n.swap()].into_iter().flatten() {
            // **Followed to the end of its chain** (`arch-scribe`'s reading of
            // the first cut): a link points one level up (§5.3d), so a page copy
            // of a kit-A main holding a nested kit-B instance links to the
            // nested copy *inside A's vault main*, and only that copy's own link
            // reaches B. Every kit on the way is on the page.
            let mut cur = Some(main);
            for _ in 0..=doc.len() {
                let Some(m) = cur else { break };
                if let Some(kit) = kit_of(m) {
                    out.insert(kit);
                }
                cur = doc.get(m).and_then(|x| x.link());
            }
        }
        stack.extend(n.children().iter().copied());
    }
    out
}

/// What the list's rows hand back to the frame: the act, the item being
/// dragged, and the item whose placement is waiting — drawn dimmed.
struct Out<'a> {
    act: &'a mut Option<Act>,
    dragging: &'a mut Option<Item>,
    busy: Option<&'a Key>,
    /// A right-click took the drag away (§15 D1033): until the button is up,
    /// the drag carries nothing and its release places nothing.
    cancelled: bool,
}

impl Out<'_> {
    /// A tile's or a result's click and drag (3A, §15 D1028): a click places at
    /// the viewport's centre, a drag carries the component under the pointer
    /// and places where it is let go — unless a right-click cancelled it, as
    /// every drag in the app is (§15 D1033). Returns how opaque the row draws —
    /// the pulse's low while its placement waits.
    fn take(&mut self, ui: &egui::Ui, resp: &egui::Response, item: &Item) -> f32 {
        if resp.clicked() {
            *self.act = Some(Act::Place(item.source.clone()));
        }
        if resp.dragged() && !self.cancelled {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            *self.dragging = Some(item.clone());
        }
        if resp.drag_stopped()
            && !self.cancelled
            && let Some(at) = ui.ctx().pointer_latest_pos()
        {
            *self.act = Some(Act::Drop(item.source.clone(), at));
        }
        if self.busy == Some(&key_of(&item.source)) {
            0.5
        } else {
            1.0
        }
    }
}

/// The largest side a drag's picture is rendered at, in pixels.
const GHOST_MAX_PX: f64 = 2048.0;

/// How far the zoom may move from the one a drag's picture was drawn at before
/// it is drawn again — a tenth, so a wheel tick re-renders and the jitter of a
/// pinch does not render every frame.
const GHOST_REZOOM: f64 = 0.1;

impl Ghost {
    /// Render `item`'s component at `zoom` on a worker — the default variant of a
    /// set, as a placement inserts (§15 D1029).
    fn start(item: &Item, doc: &Document, zoom: f64) -> Self {
        enum Scene {
            Kit(Arc<Published>),
            Here(Box<Document>),
        }
        let (scene, top) = match &item.source {
            Source::Kit { published, top, .. } => (Scene::Kit(published.clone()), *top),
            Source::Local(id) => (Scene::Here(Box::new(doc.clone())), *id),
        };
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("ondin-drag-ghost".into())
            .spawn(move || {
                let doc: &Document = match &scene {
                    Scene::Kit(p) => p.document(),
                    Scene::Here(d) => d,
                };
                // As it will land: no ancestor's transform, and a vault main
                // resolved (`kit::as_placed`) — the subject chosen first, as
                // `tiles_of` chooses it.
                let subject = tile_subject(doc, top);
                let doc = &ondin_core::kit::as_placed(doc, &[subject]);
                let res = ondin_core::Resolved::rebuild(doc);
                let got = ondin_export::extent(&res, &[subject]).and_then(|view| {
                    let (w, h) = (view.width().max(1.0), view.height().max(1.0));
                    let scale = zoom.min(GHOST_MAX_PX / w.max(h));
                    let images = super::index::prepared(doc);
                    raster_fit(doc, &res, subject, w * scale, h * scale, scale, &images)
                        .map(|p| (p, egui::vec2(w as f32, h as f32)))
                });
                let _ = tx.send(got);
            });
        Ghost {
            key: key_of(&item.source),
            rx: spawned.ok().map(|_| rx),
            picture: None,
            zoom,
        }
    }

    /// Render again for a zoom changed mid-drag past [`GHOST_REZOOM`] of the one
    /// the picture was drawn at, keeping the old picture — drawn at the new size,
    /// so stretched — until the new one lands (§15 D1035). One render at a time.
    fn follow_zoom(&mut self, item: &Item, doc: &Document, zoom: f64) {
        let off = (zoom / self.zoom - 1.0).abs();
        if self.rx.is_some() || off < GHOST_REZOOM {
            return;
        }
        let old = self.picture.take();
        *self = Ghost::start(item, doc, zoom);
        self.picture = old;
    }

    /// Take the worker's picture when it lands; keep frames coming meanwhile.
    fn pass(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(got) => {
                self.rx = None;
                self.picture = got.map(|(p, size)| (p.texture(ctx, "kit-drag-ghost".into()), size));
            }
            Err(mpsc::TryRecvError::Empty) => ctx.request_repaint(),
            Err(mpsc::TryRecvError::Disconnected) => self.rx = None,
        }
    }

    /// Wait for the worker's picture — the tests'.
    #[cfg(test)]
    fn settle(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.rx.take()
            && let Ok(got) = rx.recv_timeout(std::time::Duration::from_secs(20))
        {
            self.picture = got.map(|(p, size)| (p.texture(ctx, "kit-drag-ghost".into()), size));
        }
    }
}

/// The drag's picture under the pointer (§15 D1029): **the component at the
/// size it will land**, its world size at the canvas's zoom, once the worker has
/// it — and the tile's picture, without the tile's card, over the panel or
/// before then. Capped at half the canvas each way, so a page-sized component
/// does not cover the place it is being aimed at.
fn draw_ghost(
    ctx: &egui::Context,
    item: &Item,
    ghost: &Ghost,
    panel: egui::Rect,
    canvas: egui::Rect,
    zoom: f64,
    ppp: f32,
) {
    let Some(at) = ctx.pointer_latest_pos() else {
        return;
    };
    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("kit-tile-drag"));
    let painter = ctx.layer_painter(layer);
    match &ghost.picture {
        Some((texture, world)) if !panel.contains(at) => {
            let mut size = *world * (zoom as f32 / ppp);
            let room = canvas.size() * 0.5;
            let k = (room.x / size.x).min(room.y / size.y).min(1.0);
            size *= k;
            let rect = egui::Rect::from_center_size(at, size);
            painter.image(
                texture.id(),
                rect,
                crate::thumbs::FULL_UV,
                egui::Color32::WHITE.gamma_multiply(0.85),
            );
        }
        _ => {
            let well = egui::Rect::from_center_size(at, egui::vec2(96.0, TILE_H));
            match &item.tile {
                Some(t) => {
                    let size = t.size_vec2();
                    let k = (well.width() / size.x).min(well.height() / size.y);
                    let shown = egui::Rect::from_center_size(well.center(), size * k);
                    painter.image(t.id(), shown, crate::thumbs::FULL_UV, egui::Color32::WHITE);
                }
                None => {
                    painter.rect_filled(well, egui::CornerRadius::same(5), color::text_a(26));
                }
            }
        }
    }
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

/// Browsing: this document's own mains first, then each kit, used ones first,
/// then the library's others under their heading (3E).
///
/// ***This document* leads whenever it has anything** (§15 D1031, the
/// maintainer's idea, sparred and taken): it was 3G's, shown only when the
/// library held no kit, so a document with its own components and a kit beside
/// them had no way to insert its own from here. The picker is the one place to
/// insert any component now.
fn browse(
    ui: &mut egui::Ui,
    local: &[Item],
    shown: &[Shown],
    open: &HashSet<String>,
    out: &mut Out<'_>,
) {
    if !local.is_empty() {
        section_row(ui, "This document", &local.len().to_string());
        tiles(ui, local, out);
        ui.add_space(8.0);
    }
    if shown.is_empty() {
        if empty_note(ui) {
            *out.act = Some(Act::OpenLibrary);
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
            *out.act = Some(Act::Toggle(s.id.clone()));
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
            tiles(ui, items, out);
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
fn tiles(ui: &mut egui::Ui, items: &[Item], out: &mut Out<'_>) {
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
            // A drag's picture is drawn by `kit_picker_ui`, at the component's
            // own size (§15 D1029); it was this tile, card and all.
            let alpha = out.take(ui, &resp, item);
            let mut p = ui.painter().clone();
            p.multiply_opacity(alpha);
            draw_tile_on(&p, cell, item, resp.hovered() || resp.dragged());
        }
        ui.add_space(GAP_Y - 4.0);
    }
}

/// A tile on the grid's painter, at the opacity the caller set — the drag's
/// picture is [`draw_ghost`]'s since §15 D1029.
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
/// **Dragged as a tile is** (§15 D1028): clicking was the only way, and a drag
/// is the reach a result invites.
fn results_list(ui: &mut egui::Ui, items: &[Item], highlight: usize, out: &mut Out<'_>) {
    for (i, item) in items.iter().enumerate() {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::hover(),
        );
        // Keyed by what it places, so a list re-ranked under a drag keeps it.
        let (kit, top) = key_of(&item.source);
        let resp = ui.interact(
            rect,
            ui.id().with(("kit-result", kit, top)),
            egui::Sense::click_and_drag(),
        );
        let alpha = out.take(ui, &resp, item);
        let mut p = ui.painter().clone();
        p.multiply_opacity(alpha);
        let p = &p;
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

    /// Whether the picker lists `kit` as used by this document, or `None` when
    /// it lists no such kit — `shown_kits`' answer, for the tests of the doors
    /// outside this module that make a kit used (a swap, §15 D1019).
    pub(crate) fn kit_used(app: &mut OndinApp, ctx: &egui::Context, kit: &str) -> Option<bool> {
        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        app.shown_kits(ctx, &kits)
            .iter()
            .find(|s| s.id == kit)
            .map(|s| s.used)
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

    /// A tile let go over a frame lands inside it, centred on the release point,
    /// though the frame is turned; a click still lands at the top level; a group
    /// takes a drop only once it has a layout.
    /// flip: `centred` shifting in world space rather than the parent's → fails at
    /// "centred on the release"; `drop_container_at` refusing groups → "flex group".
    #[test]
    fn a_tile_dropped_on_a_frame_lands_inside_it() {
        use ondin_core::kurbo::{Point, Size};
        let ctx = egui::Context::default();
        let root = temp_root("drop-into");
        let (id, _set, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let frame = app.session.ids.mint();
        let doc_root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: doc_root,
            index: 0,
            kind: ondin_core::NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(
                Affine::translate((1000.0, 1000.0)) * Affine::rotate(std::f64::consts::FRAC_PI_2),
            ),
            name: None,
        }])));
        let aim = Point::new(820.0, 1150.0);
        let at = app.to_screen(aim, app.canvas_rect, 1.0);
        let source = kit_source(&mut app, &id, badge);
        app.place_kit_at(&ctx, source, at, true);
        let inst = app.session.selection.single().expect("placed");
        assert_eq!(app.session.doc.get(inst).unwrap().parent(), Some(frame));
        let c = app.session.preview_world_bounds(inst).unwrap().center();
        let landed = app.to_world(at, app.canvas_rect, 1.0);
        assert!((c - landed).hypot() < 1e-6, "centred on the release: {c:?}");

        let source = kit_source(&mut app, &id, badge);
        app.place_kit_at(&ctx, source, at, false);
        let inst = app.session.selection.single().expect("placed");
        assert_eq!(
            app.session.doc.get(inst).unwrap().parent(),
            Some(doc_root),
            "a click"
        );

        // A group with a layout takes a drop too, at the end of its flow; a plain group
        // does not.
        let (group, item) = (app.session.ids.mint(), app.session.ids.mint());
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: doc_root,
                index: 0,
                kind: ondin_core::NodeKind::Group,
                transform: Some(Affine::translate((3000.0, 3000.0))),
                name: None,
            },
            Operation::CreateNode {
                id: item,
                parent: group,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: Size::new(200.0, 200.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ])));
        let over = app.to_screen(Point::new(3100.0, 3100.0), app.canvas_rect, 1.0);
        let source = kit_source(&mut app, &id, badge);
        app.place_kit_at(&ctx, source, over, true);
        let inst = app.session.selection.single().expect("placed");
        assert_eq!(
            app.session.doc.get(inst).unwrap().parent(),
            Some(doc_root),
            "plain group"
        );
        app.undo();
        assert!(app.session.commit(Transaction(vec![Operation::SetDisplay {
            id: group,
            display: Some(ondin_core::container::Display::Flex(Default::default())),
        }])));
        let source = kit_source(&mut app, &id, badge);
        app.place_kit_at(&ctx, source, over, true);
        let inst = app.session.selection.single().expect("placed");
        assert_eq!(
            app.session.doc.get(inst).unwrap().parent(),
            Some(group),
            "flex group"
        );
        assert_eq!(
            app.session.doc.get(group).unwrap().children().last(),
            Some(&inst)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A frame whose own eye is on but whose parent is hidden, or whose part
    /// under the point its parent clips away, does not take a drop: the tile
    /// lands on the canvas, where it can be seen.
    ///
    /// flip: `drop_container_at` testing the frame's own eye → fails at "hidden
    /// parent"; descending into a clipping frame the point is outside → fails at
    /// "clipped away".
    #[test]
    fn a_tile_dropped_on_a_frame_that_cannot_be_seen_lands_on_the_canvas() {
        use ondin_core::kurbo::{Point, Size};
        let ctx = egui::Context::default();
        let root = temp_root("drop-unseen");
        let (id, _set, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let doc_root = app.session.doc.first_page();
        let frame = |id, parent, at: (f64, f64), side: f64| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: ondin_core::NodeKind::Artboard {
                size: Size::new(side, side),
            },
            transform: Some(Affine::translate(at)),
            name: None,
        };
        let [hidden, inner, clipper, overflow] = std::array::from_fn(|_| app.session.ids.mint());
        assert!(app.session.commit(Transaction(vec![
            frame(hidden, doc_root, (5000.0, 5000.0), 400.0),
            frame(inner, hidden, (0.0, 0.0), 200.0),
            Operation::SetVisible {
                id: hidden,
                visible: false,
            },
            frame(clipper, doc_root, (8000.0, 8000.0), 100.0),
            frame(overflow, clipper, (50.0, 50.0), 200.0),
        ])));
        assert!(
            app.session.doc.get(clipper).unwrap().clip(),
            "fixture: P clips"
        );
        let mut parent_of_drop = |world: Point| {
            let at = app.to_screen(world, app.canvas_rect, 1.0);
            let source = kit_source(&mut app, &id, badge);
            app.place_kit_at(&ctx, source, at, true);
            let inst = app.session.selection.single().expect("placed");
            app.session.doc.get(inst).unwrap().parent()
        };
        assert_eq!(
            parent_of_drop(Point::new(5100.0, 5100.0)),
            Some(doc_root),
            "hidden parent"
        );
        assert_eq!(
            parent_of_drop(Point::new(8080.0, 8080.0)),
            Some(overflow),
            "inside the clip, the drop still lands in the frame"
        );
        assert_eq!(
            parent_of_drop(Point::new(8200.0, 8200.0)),
            Some(doc_root),
            "clipped away"
        );
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
        let doc_root = app.session.doc.first_page();
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

        // Back over the panel: a change of mind. The frames a placement waits
        // (§15 D1030) pumped first, and nothing queued — checked on the release
        // alone it could not tell "not asked" from "asked, not yet run" (the
        // v0.5.0 release review's `[X4.2-L6-01]`).
        drag(&mut app, tile, tile + egui::vec2(60.0, 0.0));
        frame_with(&mut app, vec![]);
        frame_with(&mut app, vec![]);
        assert!(
            app.kit_placing.is_none() && app.session.selection.single().is_none(),
            "let go over the panel, nothing placed"
        );

        let to = egui::pos2(600.0, 300.0);
        drag(&mut app, tile, to);
        // A frame with the tile dimmed, then the placement (§15 D1030).
        assert!(
            app.session.selection.single().is_none(),
            "not on the release"
        );
        frame_with(&mut app, vec![]);
        frame_with(&mut app, vec![]);
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

    /// **A set's tile is its default variant** (§15 D1028): the picture is the
    /// one a click places — *Small*, first in reading order — not the set's
    /// sheet of variants.
    ///
    /// Flip, run: `tile_subject` answering the top fails, the set's 300 × 30
    /// box rendered at 200 × 20 against *Small*'s 200 × 75 — measured before
    /// tiles were drawn as placed; *Small*'s is 200 × 50 since, not re-flipped.
    #[test]
    fn a_sets_tile_is_its_default_variant() {
        let root = temp_root("set-tile");
        let (_, set, _) = file_kit(&root);
        let doc = ondin_core::io::load(&std::fs::read(root.join("atlas.ondin")).unwrap()).unwrap();
        let published = doc.published().expect("the fixture published");
        let frozen = published.document();
        let small = ondin_core::kit::default_variant(frozen, set).unwrap();
        assert_eq!(frozen.get(small).unwrap().name(), "Small", "the fixture");
        let tiles = tiles_of(frozen, &[set]);
        // *Small* as an insert lands, out of its set — whose layout stretches
        // it to 80 × 30 where it sits; it lands at the 80 × 20 it stores (the
        // v0.5.0 release review's `[X4.1-L1-02]`).
        let placed = ondin_core::kit::as_placed(frozen, &[small]);
        let res = ondin_core::Resolved::rebuild(&placed);
        let images = super::super::index::prepared(&placed);
        let alone = raster_fit(&placed, &res, small, 200.0, 104.0, 4.0, &images).unwrap();
        let tile = &tiles[&set];
        assert_eq!(
            (tile.width, tile.height),
            (alone.width, alone.height),
            "the set's tile is Small's picture"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***This document*'s pictures are drawn again only for a main that
    /// changed, once the edits settle** (§15 D1034): an edit to a layer outside
    /// every main starts no render at all, and an edit inside one re-renders that
    /// one, the other's picture kept as it was. And since §15 D1035: a main only
    /// moved renders nothing, one rotated renders, and `invalidate` — a family
    /// the document names arriving — renders every picture at once.
    ///
    /// Flip, run: comparing nothing (every main counted as changed) fails *"an
    /// unrelated edit renders nothing"*; dropping the settle wait fails *"not
    /// before the edits settle"* — asked on the edit inside A, since asked on the
    /// unrelated one first it passed under the flip, nothing rendering either way;
    /// `content_of` keeping the translation fails *"a moved main renders
    /// nothing"* (§15 D1035). The app's font hook that calls `invalidate` is not
    /// driven — a font's arrival cannot be faked headlessly.
    #[test]
    fn local_pictures_redraw_only_a_changed_main_once_settled() {
        let ctx = egui::Context::default();
        let mut ids = IdSource::new(0xBD);
        let r = ids.mint();
        let mut doc = Document::new(r);
        let [a, b, loose] = [(); 3].map(|_| ids.mint());
        doc.apply(&Transaction(vec![
            frame(a, r, 0, 40.0, "A"),
            frame(b, r, 1, 60.0, "B"),
            frame(loose, r, 2, 80.0, "Loose"),
            Operation::SetComponent {
                id: a,
                component: true,
            },
            Operation::SetComponent {
                id: b,
                component: true,
            },
        ]))
        .unwrap();
        let tops = [a, b];
        let mut tiles = LocalTiles::default();
        tiles.pass(&ctx, &doc, 1, &tops);
        assert!(tiles.busy(), "the first look renders at once");
        tiles.settle(&ctx);
        let first_b = tiles.tiles[&b].id();
        let age = |tiles: &mut LocalTiles| {
            if let Some((r, _)) = tiles.pending {
                tiles.pending = Some((r, std::time::Instant::now() - SETTLE * 2));
            }
        };

        // An edit outside every main.
        doc.apply(&Transaction(vec![Operation::SetTransform {
            id: loose,
            transform: Affine::translate((5.0, 0.0)),
        }]))
        .unwrap();
        tiles.pass(&ctx, &doc, 2, &tops);
        age(&mut tiles);
        tiles.pass(&ctx, &doc, 2, &tops);
        assert!(!tiles.busy(), "an unrelated edit renders nothing");

        // An edit inside A.
        doc.apply(&Transaction(vec![Operation::SetName {
            id: a,
            name: "A2".into(),
        }]))
        .unwrap();
        tiles.pass(&ctx, &doc, 3, &tops);
        // Asked here, on an edit that does render: an unrelated one starts
        // nothing either way, so it cannot tell a wait from no wait.
        assert!(!tiles.busy(), "not before the edits settle");
        age(&mut tiles);
        tiles.pass(&ctx, &doc, 3, &tops);
        assert!(tiles.busy(), "a changed main renders");
        tiles.settle(&ctx);
        assert_eq!(
            tiles.tiles[&b].id(),
            first_b,
            "and only it: B's picture kept"
        );
        assert!(tiles.tiles.contains_key(&a));

        // A moved on the canvas draws the same picture (§15 D1035) …
        let mut rev = 3;
        let mut edit = |doc: &mut Document, tiles: &mut LocalTiles, t: Affine| {
            rev += 1;
            doc.apply(&Transaction(vec![Operation::SetTransform {
                id: a,
                transform: t,
            }]))
            .unwrap();
            tiles.pass(&ctx, doc, rev, &tops);
            age(tiles);
            tiles.pass(&ctx, doc, rev, &tops);
            rev
        };
        edit(&mut doc, &mut tiles, Affine::translate((300.0, 40.0)));
        assert!(!tiles.busy(), "a moved main renders nothing");
        // … and one rotated does not.
        let rev = edit(
            &mut doc,
            &mut tiles,
            Affine::translate((300.0, 40.0)) * Affine::rotate(0.5),
        );
        assert!(tiles.busy(), "a rotated main renders");
        tiles.settle(&ctx);

        // A family the document names arriving redraws every picture, at once.
        let before_b = tiles.tiles[&b].id();
        tiles.invalidate();
        tiles.pass(&ctx, &doc, rev, &tops);
        assert!(tiles.busy(), "a font arriving renders, without the wait");
        tiles.settle(&ctx);
        assert_ne!(tiles.tiles[&b].id(), before_b, "B's picture too");
    }

    /// **A kit reached through another kit's component counts as used** (§15
    /// D1028's chain walk): *Beacon*'s main *Card* holds an instance of
    /// *Atlas*'s *Badge*; placing *Card* puts no link to *Atlas* on the page —
    /// the nested copy links to the copy inside *Beacon*'s vault main, one level
    /// up — and *Atlas* is used all the same.
    ///
    /// Flip, run: the walk stopping at the first link fails *"Atlas is used
    /// through Beacon"*.
    #[test]
    fn a_kit_nested_in_another_kits_main_counts_as_used() {
        let ctx = egui::Context::default();
        let root = temp_root("nested-used");
        let (atlas, _, badge) = file_kit(&root);
        let atlas_doc =
            ondin_core::io::load(&std::fs::read(root.join("atlas.ondin")).unwrap()).unwrap();
        let atlas_pub = atlas_doc.published().unwrap().clone();

        // *Beacon*: a frame *Card*, a main, holding an Atlas *Badge* instance.
        let mut ids = IdSource::new(0xBE);
        let r = ids.mint();
        let mut doc = Document::new(r);
        let card = ids.mint();
        doc.apply(&Transaction(vec![
            frame(card, r, 0, 200.0, "Card"),
            Operation::SetComponent {
                id: card,
                component: true,
            },
        ]))
        .unwrap();
        let (tx, _) =
            ondin_core::kit::insert(&doc, &mut ids, &atlas, 4, &atlas_pub, badge, card).unwrap();
        doc.apply(&tx).unwrap();
        let path = crate::library::store::file_document(&root, None, "Beacon", &mut doc).unwrap();
        let mut meta = doc.meta().clone();
        meta.kit = Some(KitMark {
            version: 1,
            ..KitMark::default()
        });
        doc.set_meta(meta);
        doc.set_published(ondin_core::kit::freeze(&doc));
        crate::library::store::write_document(&path, &doc).unwrap();
        let beacon = doc.meta().id.clone().unwrap();

        let mut app = app_on(&ctx, root.clone());
        let source = kit_source(&mut app, &beacon, card);
        app.place_from_picker(&ctx, source);
        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        let used: Vec<(String, bool)> = app
            .shown_kits(&ctx, &kits)
            .iter()
            .map(|s| (s.name.clone(), s.used))
            .collect();
        assert!(
            used.contains(&("Beacon".to_string(), true)),
            "the fixture: Beacon placed: {used:?}"
        );
        assert!(
            used.contains(&("Atlas".to_string(), true)),
            "Atlas is used through Beacon: {used:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A right-click takes a picker drag away** (§15 D1033), as it does every
    /// drag in the app: the picture goes on the press, and the primary's release
    /// over the canvas places nothing. Driven the way `OndinApp::ui` drives it —
    /// `cancel_gesture` on the secondary press, before the panels, and
    /// `gesture_cancelled` cleared once no button is down.
    ///
    /// The right-click is **pressed and released in one frame**, the fast click
    /// that clears egui's `dragged_id` before `cancel_gesture` reads it.
    ///
    /// Flip, run: `Out::take` ignoring `cancelled` fails *"the release places
    /// nothing"*. `cancel_gesture` without the picker's term **passed** while the
    /// right-click spanned two frames — `dragged_id` carried it — and fails *"the
    /// release places nothing"* with the one-frame click, which is why the test
    /// drives that one; since the menu assertion joined (§15 D1035) it fails
    /// earlier, at *"the cancelling right-click opens no menu"*.
    #[test]
    fn a_right_click_takes_a_picker_drag_away() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("drag-cancel");
        let mut app = app_on(&ctx, root.clone());
        let main = app.session.ids.mint();
        let doc_root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            frame(main, doc_root, 0, 80.0, "Button"),
            Operation::SetComponent {
                id: main,
                component: true,
            },
        ])));
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |_| {
                    if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Secondary)) {
                        app.cancel_gesture(&ctx);
                    }
                    app.kit_picker_ui(&ctx);
                    if ctx.input(|i| !i.pointer.any_down()) {
                        app.gesture_cancelled = false;
                    }
                },
            );
            out.shapes
        };
        let button = |b, at, pressed| egui::Event::PointerButton {
            pos: at,
            button: b,
            pressed,
            modifiers: Default::default(),
        };
        app.toggle_kit_picker();
        run(&mut app, vec![]);
        let shapes = run(&mut app, vec![]);
        let tile = shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Button" => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("the tile")
            - egui::vec2(0.0, 30.0);
        let to = egui::pos2(600.0, 300.0);
        run(&mut app, vec![egui::Event::PointerMoved(tile)]);
        run(
            &mut app,
            vec![button(egui::PointerButton::Primary, tile, true)],
        );
        for k in 1..=4 {
            let at = tile + (to - tile) * (k as f32 / 4.0);
            run(&mut app, vec![egui::Event::PointerMoved(at)]);
        }
        assert!(
            app.kit_picker.as_ref().unwrap().dragging(),
            "the fixture: a component is being carried"
        );
        // Press and release in one frame — a click under ~16 ms, which clears
        // egui's `dragged_id` before `cancel_gesture` can read it.
        run(
            &mut app,
            vec![
                button(egui::PointerButton::Secondary, to, true),
                button(egui::PointerButton::Secondary, to, false),
            ],
        );
        assert!(
            !app.kit_picker.as_ref().unwrap().dragging(),
            "the picture goes on the right-click"
        );
        // The right-click spends itself on the cancel, opening no canvas menu
        // over the drop point — asked of the door the canvas's own release goes
        // through (§15 D1035; the gate is R1's, §15 D168).
        app.open_context_menu(&ctx, crate::menu::Target::Canvas, None);
        assert!(
            app.context_menu.is_none(),
            "the cancelling right-click opens no menu"
        );
        run(
            &mut app,
            vec![button(egui::PointerButton::Primary, to, false)],
        );
        for _ in 0..3 {
            run(&mut app, vec![]);
        }
        assert!(
            app.session.selection.single().is_none() && app.kit_placing.is_none(),
            "the release places nothing"
        );
        assert!(!app.gesture_cancelled, "and the next drag starts fresh");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A placement waiting its dimmed frame survives the picker closing**
    /// (§15 D1030, `arch-scribe`'s reading of the first cut): `Escape` in the
    /// wait used to drop the click with the picker that held it.
    ///
    /// Flip, run: the due test without its `|| closed` fails *"the click
    /// still places"*.
    #[test]
    fn a_placement_survives_the_picker_closing_in_its_wait() {
        let ctx = egui::Context::default();
        let root = temp_root("close-wait");
        let (id, _, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        app.toggle_kit_picker();
        let source = kit_source(&mut app, &id, badge);
        app.ask_place(source, None);
        app.kit_picker = None;
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let inst = app
            .session
            .selection
            .single()
            .expect("the click still places");
        assert!(app.session.doc.get(inst).unwrap().link().is_some());
        assert!(app.kit_placing.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A kit reads as used only while an instance of it is on the page**
    /// (§15 D1028): placed, it is used; its instance deleted, it is listed under
    /// *In your library, not used here* again — though its vendored main stays
    /// in the vault, which is what the old reading counted.
    ///
    /// Flip, run: `shown_kits` reading the vault for `used` again fails
    /// *"deleted, not used"*.
    #[test]
    fn a_kit_is_used_only_while_an_instance_is_on_the_page() {
        let ctx = egui::Context::default();
        let root = temp_root("used");
        let (id, _, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let used = |app: &mut OndinApp| {
            let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
            app.shown_kits(&ctx, &kits)
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.used)
        };
        assert_eq!(used(&mut app), Some(false), "the control: never used");
        let source = kit_source(&mut app, &id, badge);
        app.place_from_picker(&ctx, source);
        assert_eq!(used(&mut app), Some(true), "placed, used");
        let inst = app.session.selection.single().unwrap();
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: inst }]))
        );
        assert!(
            app.session.doc.vault().is_some(),
            "the fixture: the vendored main stays"
        );
        assert_eq!(used(&mut app), Some(false), "deleted, not used");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ***This document* is listed above the kits, with pictures, and a search
    /// result drags onto the canvas at the component's own size** (§15 D1028,
    /// D1029, D1031). The drag's picture is the component's world size at the
    /// canvas's zoom — *Badge*, 24 × 30, at zoom 2 on a 1-point display, 48 × 60
    /// — and let go over the canvas, it places after one dimmed frame (D1030).
    ///
    /// Flip, run: `draw_ghost` drawing the tile's picture instead (its fallback
    /// arm) fails *"the ghost is the component at canvas size"*, a 42 × 52 tile
    /// at the pointer; `Out::take` never dimming fails *"the waiting result
    /// dims"*, 255 against 251.
    #[test]
    fn local_mains_lead_and_a_result_drags_at_the_components_size() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("result-drag");
        let (id, _, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let mine = app.session.ids.mint();
        let doc_root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            frame(mine, doc_root, 0, 50.0, "Mine"),
            Operation::SetComponent {
                id: mine,
                component: true,
            },
        ])));
        app.session.camera.zoom = 2.0;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let run = |app: &mut OndinApp, events: Vec<egui::Event>| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |_| app.kit_picker_ui(&ctx),
            )
            .shapes
        };
        let text_at = |shapes: &[egui::epaint::ClippedShape], want: &str| {
            shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == want => {
                    Some(t.pos + t.galley.size() / 2.0)
                }
                _ => None,
            })
        };

        app.toggle_kit_picker();
        app.library.refresh();
        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        for k in &kits {
            app.kit_shelf.ask(k);
        }
        app.kit_shelf.settle();
        run(&mut app, vec![]);
        app.kit_picker.as_mut().unwrap().local.settle(&ctx);
        let shapes = run(&mut app, vec![]);
        assert!(
            text_at(&shapes, "This document").is_some(),
            "This document is listed beside a kit"
        );
        assert!(text_at(&shapes, "Mine").is_some(), "with its main");
        assert!(
            app.kit_picker
                .as_ref()
                .unwrap()
                .local
                .tiles
                .contains_key(&mine),
            "and its picture"
        );

        app.kit_picker.as_mut().unwrap().query = "badge".into();
        run(&mut app, vec![]);
        let shapes = run(&mut app, vec![]);
        let row = text_at(&shapes, "Badge").expect("the result");
        let press = |at, pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let to = egui::pos2(600.0, 300.0);
        run(&mut app, vec![egui::Event::PointerMoved(row)]);
        run(&mut app, vec![press(row, true)]);
        for k in 1..=4 {
            let at = row + (to - row) * (k as f32 / 4.0);
            run(&mut app, vec![egui::Event::PointerMoved(at)]);
        }
        app.kit_picker
            .as_mut()
            .unwrap()
            .ghost
            .as_mut()
            .expect("a drag starts a ghost")
            .settle(&ctx);
        let shapes = run(&mut app, vec![egui::Event::PointerMoved(to)]);
        let images: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(
                    egui::Rect::from_points(&m.vertices.iter().map(|v| v.pos).collect::<Vec<_>>()),
                ),
                _ => None,
            })
            .collect();
        assert!(
            images.iter().any(|r| r.center().distance(to) < 0.5
                && (r.size() - egui::vec2(48.0, 60.0)).length() < 0.5),
            "the ghost is the component at canvas size: {images:?}"
        );

        // A zoom mid-drag renders it again, the old picture standing meanwhile
        // (§15 D1035).
        fn ghost_of(app: &OndinApp) -> &Ghost {
            app.kit_picker.as_ref().unwrap().ghost.as_ref().unwrap()
        }
        let first = ghost_of(&app).picture.as_ref().unwrap().0.id();
        app.session.camera.zoom = 3.0;
        run(&mut app, vec![egui::Event::PointerMoved(to)]);
        assert!(
            ghost_of(&app).rx.is_some() && ghost_of(&app).picture.is_some(),
            "a re-render asked, the old picture kept meanwhile"
        );
        app.kit_picker
            .as_mut()
            .unwrap()
            .ghost
            .as_mut()
            .unwrap()
            .settle(&ctx);
        let zoomed = run(&mut app, vec![egui::Event::PointerMoved(to)]);
        assert_ne!(
            ghost_of(&app).picture.as_ref().unwrap().0.id(),
            first,
            "a new picture"
        );
        let sizes: Vec<egui::Vec2> = zoomed
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(
                    egui::Rect::from_points(&m.vertices.iter().map(|v| v.pos).collect::<Vec<_>>())
                        .size(),
                ),
                _ => None,
            })
            .collect();
        assert!(
            sizes
                .iter()
                .any(|s| (*s - egui::vec2(72.0, 90.0)).length() < 0.5),
            "drawn at the new zoom: {sizes:?}"
        );

        let ink = |shapes: &[egui::epaint::ClippedShape]| {
            shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == "Badge" => {
                    Some(t.galley.rows[0].visuals.mesh.vertices[0].color.a())
                }
                _ => None,
            })
        };
        let resting = ink(&shapes).expect("the result's name");
        run(&mut app, vec![press(to, false)]);
        let waiting = run(&mut app, vec![]);
        assert!(
            app.session.selection.single().is_none(),
            "the frame that shows the dim places nothing yet"
        );
        let dim = ink(&waiting).expect("the result's name");
        // Half, not a hover's few points: the flip (no dim) reads 255 here.
        assert!(
            (dim as f32) < resting as f32 * 0.6,
            "the waiting result dims: {dim} against {resting}"
        );
        run(&mut app, vec![]);
        let inst = app
            .session
            .selection
            .single()
            .expect("dropped on the canvas");
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        assert_eq!(app.session.doc.get(main).unwrap().origin().unwrap().kit, id);
        let _ = badge;
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
        // The frames a placement waits (§15 D1030), then nothing queued or
        // placed: checked on the `Enter`'s own frame it passed whatever the
        // guard did (the v0.5.0 release review's `[X4.2-L6-01]`).
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        }
        assert!(
            app.kit_placing.is_none(),
            "not the picker's Enter: nothing queued"
        );
        assert_eq!(app.session.doc, before, "not the picker's Enter");

        ctx.memory_mut(|m| m.request_focus(field));
        let _ = ctx.run_ui(enter(), |_| app.kit_picker_ui(&ctx));
        // A frame with the result dimmed, then the placement (§15 D1030).
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        }
        let inst = app.session.selection.single().expect("Enter placed it");
        let main = app.session.doc.get(inst).unwrap().link().unwrap();
        assert_eq!(app.session.doc.get(main).unwrap().origin().unwrap().kit, id);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A main's tile and drag picture are drawn as it lands** (the v0.5.0
    /// release review's `[X4.1-L1-02]`, with `[X4.1-L1-04]`). *Badge* is 80 ×
    /// 30 in a frame rotated 90° and scaled 2×. The picture was drawn where it
    /// sat — 60 × 160, turned — and the placement landed 80 × 30 and level; and
    /// a vault main, which nothing resolves where it sits, dragged as an empty
    /// box. Both now draw from `kit::as_placed`.
    ///
    /// Flip, run: the ghost drawing from the document as it is fails *"the
    /// size it lands at"* with 60 × 160 against 80 × 30. The vault half was
    /// not flipped on its own; the review's probe measured `None` there.
    #[test]
    fn a_mains_pictures_are_drawn_as_it_lands() {
        let ctx = egui::Context::default();
        let mut ids = IdSource::new(0xDF);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [p, badge] = [(); 2].map(|_| ids.mint());
        doc.apply(&Transaction(vec![
            frame(p, root, 0, 200.0, "Holder"),
            frame(badge, p, 0, 80.0, "Badge"),
            Operation::SetComponent {
                id: badge,
                component: true,
            },
            Operation::SetTransform {
                id: p,
                transform: Affine::rotate(std::f64::consts::FRAC_PI_2).then_scale(2.0),
            },
        ]))
        .unwrap();
        let item = |source| Item {
            source,
            name: "Badge".into(),
            set: false,
            kit: None,
            tile: None,
            aliases: Vec::new(),
        };
        let mut ghost = Ghost::start(&item(Source::Local(badge)), &doc, 1.0);
        ghost.settle(&ctx);
        let (_, size) = ghost.picture.expect("a picture");
        let stored = match doc.get(badge).unwrap().kind() {
            NodeKind::Artboard { size } => *size,
            _ => unreachable!(),
        };
        assert_eq!(
            (size.x as f64, size.y as f64),
            (stored.width, stored.height),
            "the size it lands at"
        );

        // A vault main: an unavailable kit's copy, listed by itself (3I).
        let mut d = Document::new(IdSource::new(0xD1).mint());
        let mut dids = IdSource::new(0xD2);
        let mut kdoc = doc.clone();
        kdoc.set_meta(ondin_core::meta::DocumentMeta {
            id: Some("5eed0000000000000000000000000009".into()),
            kit: Some(KitMark::default()),
            ..Default::default()
        });
        let published = ondin_core::kit::freeze(&kdoc).unwrap();
        let droot = d.first_page();
        let (tx, inst) = ondin_core::kit::insert(
            &d,
            &mut dids,
            "5eed0000000000000000000000000009",
            1,
            &published,
            badge,
            droot,
        )
        .unwrap();
        d.apply(&tx).unwrap();
        let vaulted = d.get(inst).unwrap().link().unwrap();
        let mut ghost = Ghost::start(&item(Source::Local(vaulted)), &d, 1.0);
        ghost.settle(&ctx);
        assert!(ghost.picture.is_some(), "a vault main draws");
    }

    /// **A tile let go over a panel floating on the canvas places nothing**
    /// (the v0.5.0 release review's `[X4.1-L1-03]`): inside the canvas's rect,
    /// off the picker, but over the inspector's area. This asks the predicate;
    /// that the call site hands it `Context::layer_id_at` is read, not driven —
    /// the picker's drag test runs the picker alone, with no inspector drawn.
    ///
    /// Flip, run: `under` ignored fails *"over a floating panel"*.
    #[test]
    fn a_tile_let_go_over_a_floating_panel_places_nothing() {
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let panel = egui::Rect::from_min_size(egui::pos2(0.0, 300.0), egui::vec2(200.0, 300.0));
        let at = egui::pos2(700.0, 100.0);
        let inspector = egui::LayerId::new(egui::Order::Middle, egui::Id::new("inspector"));
        assert!(super::dropped_on_canvas(at, canvas, panel, None));
        assert!(super::dropped_on_canvas(
            at,
            canvas,
            panel,
            Some(egui::LayerId::background())
        ));
        assert!(
            !super::dropped_on_canvas(at, canvas, panel, Some(inspector)),
            "over a floating panel"
        );
    }

    /// **A hugging frame is centred by the box layout gives it** (the v0.5.0
    /// release review's `[R1-L2-01]`): a flex frame stored at 10×10 that hugs a
    /// 120×40 rect. Centring its stored box put the real one 55 px right and
    /// 15 px down of the aim.
    ///
    /// Flip, run: measuring a frame by its stored size again fails *"the laid-out
    /// box is centred on the aim"*, centre (555, 315) against (500, 300) — the
    /// review's 55 and 15 exactly.
    #[test]
    fn a_hugging_frame_is_centred_by_its_laid_out_box() {
        use ondin_core::container::{Dimension, Display, Flex};
        let mut ids = IdSource::new(0xDB);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let [f, r] = [(); 2].map(|_| ids.mint());
        let item = ondin_core::container::LayoutItem {
            width: Dimension::FitContent,
            height: Dimension::FitContent,
            ..Default::default()
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: f,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(10.0, 10.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: r,
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
        let laid = ondin_core::Resolved::rebuild(&doc).world_bounds(f).unwrap();
        assert_eq!(
            laid.size(),
            Size::new(120.0, 40.0),
            "fixture: the frame hugs"
        );

        let aim = ondin_core::kurbo::Point::new(500.0, 300.0);
        let op = super::centred(&doc, &Transaction(vec![]), f, aim).unwrap();
        doc.apply(&Transaction(vec![op])).unwrap();
        let b = ondin_core::Resolved::rebuild(&doc).world_bounds(f).unwrap();
        assert_eq!(b.center(), aim, "the laid-out box is centred on the aim");
    }

    /// **A kit the library lost is still listed by its copies, and its set
    /// places its default variant** (3I; the v0.5.0 release review's
    /// `[X4.2-L6-04]` — no picker test made a kit unavailable). *Atlas*' set and
    /// *Badge* are placed, both instances deleted — the copies stay in the vault
    /// — and the kit's file is removed. With no instance on the page and no file,
    /// only `shown_kits`' lost list keeps it: *Unavailable*, not used, its note
    /// counting the two copies, its tiles the vault's own. Placed from there, the
    /// set inserts *Small*, its default variant, as a kit placement does.
    ///
    /// Flip, run: the lost list left empty fails *"listed by its copies"*. Flip,
    /// run: `place_kit_at`'s `Source::Local` placing a set as itself (its
    /// `default_variant` arm dropped) fails *"the default variant"* with `None`:
    /// a copy of the set itself, a plain frame linked to nothing.
    #[test]
    fn a_kit_the_library_lost_is_listed_by_its_copies() {
        let ctx = egui::Context::default();
        let root = temp_root("lost");
        let (id, set, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        for top in [set, badge] {
            let source = kit_source(&mut app, &id, top);
            app.place_from_picker(&ctx, source);
            let inst = app.session.selection.single().unwrap();
            assert!(
                app.session
                    .commit(Transaction(vec![Operation::DeleteNode { id: inst }]))
            );
        }
        std::fs::remove_file(root.join("atlas.ondin")).unwrap();
        app.library.refresh();
        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        assert!(kits.is_empty(), "the fixture: the library has no kit");

        let shown = app.shown_kits(&ctx, &kits);
        let lost = shown
            .iter()
            .find(|s| s.id == id)
            .expect("listed by its copies");
        assert_eq!(
            (lost.name.as_str(), lost.status.as_str(), lost.used),
            ("Atlas", "Unavailable", false)
        );
        assert_eq!(
            lost.note.as_deref(),
            Some(
                "Not found in your library folder. The 2 components already in this document \
                 keep working, and you can still insert them."
            )
        );
        let items: Vec<(String, bool)> = lost
            .groups
            .iter()
            .flat_map(|(_, i)| i)
            .map(|i| (i.name.clone(), i.set))
            .collect();
        assert_eq!(
            items,
            vec![("Button".to_string(), true), ("Badge".to_string(), false)]
        );

        let vault_set = match lost.groups[0].1[0].source {
            Source::Local(v) => v,
            Source::Kit { .. } => panic!("a lost kit's copies are local"),
        };
        app.place_from_picker(&ctx, Source::Local(vault_set));
        let inst = app.session.selection.single().expect("placed");
        let doc = &app.session.doc;
        let main = doc.get(inst).unwrap().link();
        assert_eq!(
            main.and_then(|m| doc.get(m)).map(|n| n.name()),
            Some("Small"),
            "the default variant"
        );
        assert!(main.is_some_and(|m| doc.in_vault(m)));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A kit that never published, and one whose file cannot be read, say so**
    /// (3H; the v0.5.0 release review's `[X4.2-L6-04]` — no fixture had either).
    /// *Beacon* is marked a kit and has published nothing; *Atlas*' file is
    /// overwritten with bytes no document parses after the library listed it.
    /// Each kit's header reads its status and the open kit its note instead of
    /// tiles.
    ///
    /// Flip, run: the two arms' statuses swapped fails *"never published"* with
    /// *Unreadable*.
    #[test]
    fn a_kit_not_published_and_one_unreadable_say_so() {
        let ctx = egui::Context::default();
        let root = temp_root("states");
        let (atlas, _, _) = file_kit(&root);
        let mut ids = IdSource::new(0xBC);
        let mut doc = Document::new(ids.mint());
        let path = crate::library::store::file_document(&root, None, "Beacon", &mut doc).unwrap();
        let mut meta = doc.meta().clone();
        meta.kit = Some(KitMark::default());
        doc.set_meta(meta);
        crate::library::store::write_document(&path, &doc).unwrap();
        let beacon = doc.meta().id.clone().unwrap();
        let mut app = app_on(&ctx, root.clone());
        std::fs::write(root.join("atlas.ondin"), b"not a document").unwrap();

        let kits = kits_in(&app.library, app.session.doc.meta().id.as_deref());
        assert_eq!(kits.len(), 2, "the fixture: both listed");
        for k in &kits {
            app.kit_shelf.ask(k);
        }
        app.kit_shelf.settle();
        let shown = app.shown_kits(&ctx, &kits);
        let read = |id: &str| {
            let s = shown.iter().find(|s| s.id == id).unwrap();
            (s.status.clone(), s.note.clone(), s.groups.len())
        };
        assert_eq!(
            read(&beacon),
            (
                "not published".to_string(),
                Some(
                    "Nothing published yet. Its author publishes from the top bar of Beacon."
                        .to_string()
                ),
                0
            ),
            "never published"
        );
        assert_eq!(
            read(&atlas),
            (
                "Unreadable".to_string(),
                Some("This kit's file could not be read.".to_string()),
                0
            ),
            "unreadable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Search ranks a name that starts with the query, then one that holds
    /// it, then a set found through its variants — this document's own first
    /// among equal names — and stops at `MAX_RESULTS`** (3F; §15 D1031; the
    /// v0.5.0 release review's `[X4.2-L6-04]`, which found the only search test
    /// holding a single result). Asked of `search` itself, over items built here.
    ///
    /// Flip, run: every match scored 0 fails *"ranked"*, the five sorted by name
    /// alone. Flip, run: the kits chained before this document's own fails
    /// *"ranked"* too, the kit's *Badge* first. Flip, run: the `take(MAX_RESULTS)`
    /// dropped fails *"capped"* with 45.
    #[test]
    fn search_ranks_prefix_then_substring_then_variant_and_caps() {
        let any = IdSource::new(0xEA).mint();
        let item = |name: &str, aliases: &[&str], here: bool| Item {
            source: Source::Local(any),
            name: name.into(),
            set: false,
            kit: Some((if here { "This document" } else { "Atlas" }.into(), here)),
            tile: None,
            aliases: std::iter::once(name.to_lowercase())
                .chain(aliases.iter().map(|a| a.to_string()))
                .collect(),
        };
        let shown = |items: Vec<Item>| Shown {
            id: "k".into(),
            name: "Atlas".into(),
            status: String::new(),
            used: true,
            groups: vec![("Atlas".into(), items)],
            note: None,
        };
        let local = vec![item("Bar", &[], true), item("Badge", &[], true)];
        let kit = shown(vec![
            item("Kebab", &[], false),
            item("Button", &["basic"], false),
            item("Badge", &[], false),
            item("Card", &[], false),
        ]);
        let got: Vec<(String, bool)> = search("ba", &local, std::slice::from_ref(&kit))
            .iter()
            .map(|i| (i.name.clone(), i.kit.as_ref().unwrap().1))
            .collect();
        let want = |n: &str, here: bool| (n.to_string(), here);
        assert_eq!(
            got,
            vec![
                want("Badge", true),
                want("Badge", false),
                want("Bar", true),
                want("Kebab", false),
                want("Button", false),
            ],
            "ranked"
        );

        let many = shown(
            (0..45)
                .map(|i| item(&format!("Icon {i:02}"), &[], false))
                .collect(),
        );
        assert_eq!(
            search("icon", &[], std::slice::from_ref(&many)).len(),
            MAX_RESULTS,
            "capped"
        );
    }

    /// **`↓` moves the highlight and `Enter` places what it lit** (3F; the
    /// v0.5.0 release review's `[X4.2-L6-04]`: the arrows were never pressed).
    /// *b* finds *Badge* and *Button*, both by their names' start, in that
    /// order; one `↓` with the field focused, then `Enter`, places *Button* —
    /// its default variant, *Small*.
    ///
    /// Flip, run: `ArrowDown` not moving the highlight (its `+ 1` dropped) fails
    /// *"the lit result placed"* with *Badge*.
    #[test]
    fn arrow_down_lights_the_next_result_and_enter_places_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("arrows");
        let (_, _, _) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let key = |key| {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            });
            input
        };
        app.toggle_kit_picker();
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        app.kit_shelf.settle();
        app.kit_picker.as_mut().unwrap().query = "b".into();
        let out = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let _ = out;
        let out = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let rows: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) if ["Badge", "Button"].contains(&t.galley.text()) => {
                    Some(t.galley.text().to_string())
                }
                _ => None,
            })
            .collect();
        assert_eq!(rows, ["Badge", "Button"], "the fixture: two results");
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
        let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        let _ = ctx.run_ui(key(egui::Key::ArrowDown), |_| app.kit_picker_ui(&ctx));
        let _ = ctx.run_ui(key(egui::Key::Enter), |_| app.kit_picker_ui(&ctx));
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        }
        let inst = app.session.selection.single().expect("Enter placed one");
        let doc = &app.session.doc;
        let main = doc.get(inst).unwrap().link().unwrap();
        assert_eq!(
            doc.get(main).unwrap().name(),
            "Small",
            "the lit result placed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A second placement asked while one waits is dropped** (§15 D1030 — *"as
    /// a second click on a busy button is"*; the v0.5.0 release review's
    /// `[X4.2-L6-04]`: only the `cfg(test)` door, which bypasses the wait, had
    /// ever placed twice). *Badge* is asked, then the set, before a frame; the
    /// frames that follow place *Badge* alone.
    ///
    /// Flip, run: `ask_place` replacing the waiting ask (its `is_none` guard
    /// dropped) fails *"the first ask places"* with *Small*.
    #[test]
    fn a_second_ask_while_one_waits_is_dropped() {
        let ctx = egui::Context::default();
        let root = temp_root("second-ask");
        let (id, set, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        app.toggle_kit_picker();
        let first = kit_source(&mut app, &id, badge);
        let second = kit_source(&mut app, &id, set);
        app.ask_place(first, None);
        app.ask_place(second, None);
        for _ in 0..3 {
            let _ = ctx.run_ui(Default::default(), |_| app.kit_picker_ui(&ctx));
        }
        let doc = &app.session.doc;
        let page: Vec<NodeId> = doc.get(doc.first_page()).unwrap().children().to_vec();
        assert_eq!(page.len(), 1, "one placement: {page:?}");
        let main = doc.get(page[0]).unwrap().link().unwrap();
        assert_eq!(
            doc.get(main).unwrap().name(),
            "Badge",
            "the first ask places"
        );
        assert!(app.kit_placing.is_none(), "and nothing waits after it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A drag's picture is capped at half the canvas each way** (§15 D1029 —
    /// *"so a page-sized component does not cover the place it is being aimed
    /// at"*; the v0.5.0 release review's `[X4.2-L6-04]`: the ghost tests used a
    /// 24 × 30 *Badge*, far under it). A 2000 × 30 main at zoom 1 over an 800 ×
    /// 600 canvas draws 400 × 6 — scaled whole, its proportions kept.
    ///
    /// Flip, run: the cap deleted (`k` fixed at 1) fails *"half the canvas"*
    /// with 2000 × 30.
    #[test]
    fn a_drags_picture_is_capped_at_half_the_canvas() {
        let ctx = egui::Context::default();
        let mut ids = IdSource::new(0xCA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let wide = ids.mint();
        doc.apply(&Transaction(vec![
            frame(wide, root, 0, 2000.0, "Hero"),
            Operation::SetComponent {
                id: wide,
                component: true,
            },
        ]))
        .unwrap();
        let item = Item {
            source: Source::Local(wide),
            name: "Hero".into(),
            set: false,
            kit: None,
            tile: None,
            aliases: Vec::new(),
        };
        let mut ghost = Ghost::start(&item, &doc, 1.0);
        ghost.settle(&ctx);
        assert_eq!(
            ghost.picture.as_ref().map(|(_, s)| *s),
            Some(egui::vec2(2000.0, 30.0)),
            "the fixture: the picture's world size"
        );
        let canvas = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let panel = egui::Rect::from_min_size(egui::pos2(0.0, 500.0), egui::vec2(100.0, 100.0));
        let at = egui::pos2(400.0, 300.0);
        let mut sizes = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(canvas),
                    events: vec![egui::Event::PointerMoved(at)],
                    ..Default::default()
                },
                |_| draw_ghost(&ctx, &item, &ghost, panel, canvas, 1.0, 1.0),
            );
            sizes = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some(
                        egui::Rect::from_points(
                            &m.vertices.iter().map(|v| v.pos).collect::<Vec<_>>(),
                        )
                        .size(),
                    ),
                    _ => None,
                })
                .collect();
        }
        assert!(
            sizes.len() == 1 && (sizes[0] - egui::vec2(400.0, 6.0)).length() < 0.5,
            "half the canvas: {sizes:?}"
        );
    }
}
