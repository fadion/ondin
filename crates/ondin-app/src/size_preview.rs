//! The size preview: a card in the minimap's corner that keeps the selected layers
//! drawn at several fixed pixel sizes at once, on a light and a dark ground, and
//! redraws them on a worker as they are edited, drags included. A small icon is
//! judged at the sizes it ships at without zooming back and forth.
//!
//! *View ▸ Panels ▸ Size preview*, `Ctrl+Shift+Y`; off by default. While it shows,
//! the minimap is hidden, not switched off. None of it is an edit: the sizes,
//! grounds and pixel toggle are prefs, the pin is view state.

use crate::app::OndinApp;
use crate::job::Job;
use crate::prefs::{SizeGround, SizePreviewPrefs};
use crate::theme::{self, color, icon};
use crate::ui;
use ondin_core::kurbo::Rect;
use ondin_core::{Document, NodeId, Operation, Resolved, Transaction};
use ondin_render::{ImageStore, RenderOverrides, VelloCpuRenderer, Viewport};

/// The sizes the ⋯ menu offers, smallest first.
pub(crate) const SIZE_CHOICES: [u32; 10] = [12, 16, 18, 20, 24, 28, 32, 40, 48, 64];
/// The most sizes ticked at once: six keeps the card under a third of a laptop
/// canvas's width.
const MAX_SIZES: usize = 6;
/// The most layers shown, each a row; past this a footer counts the rest.
const MAX_ROWS: usize = 5;
/// The card's padding and the gap between its parts, the minimap's.
const PAD: f32 = 6.0;
/// The head's height, a list row's.
const HEAD_H: f32 = ui::MENU_ROW_H;
/// The air a tile's column adds round its N points across.
const TILE_AIR: f32 = 28.0;
/// The air a ground strip adds round its tallest tile.
const STRIP_AIR: f32 = 20.0;
/// The narrowest the card's inside gets, so the head's name and its four controls
/// fit however few sizes are ticked.
const MIN_INNER_W: f32 = 244.0;
/// The size-caption row under the tiles.
const CAPS_H: f32 = 14.0;
/// The sizes menu's narrowest: its labels are two digits, and a list sized to
/// them alone would stand narrower than its tick column is wide.
const MENU_W: f32 = 80.0;
/// The loupe's magnification of a pixel tile: enough to count a 12's pixels, and
/// a 64 tile still stands only 320 points.
const LOUPE_ZOOM: f32 = 5.0;
/// Between the card and the status bar or the loupe beside it, the minimap's.
pub(crate) const GAP: f32 = 8.0;

/// One render's subject and settings: when any part differs from what is shown,
/// the tiles are stale.
#[derive(Clone, Debug, PartialEq)]
struct Want {
    /// Each row's layer and the world box its tiles frame.
    rows: Vec<(NodeId, Rect)>,
    sizes: Vec<u32>,
    pixels: bool,
    /// Device pixels per point, bit for bit, since a vector tile renders at it.
    ppp: u32,
    revision: u64,
}

/// A worker's answer: the tiles it drew and the image store it used, handed back
/// so the next render decodes nothing it already has.
struct Landed {
    want: Want,
    /// Per row, per size: straight-alpha RGBA and its side in pixels.
    tiles: Vec<Vec<(Vec<u8>, u32)>>,
    store: ImageStore,
}

/// The tiles on screen.
struct Shown {
    want: Want,
    textures: Vec<Vec<egui::TextureHandle>>,
}

/// The card's state between frames.
#[derive(Default)]
pub(crate) struct SizePreview {
    /// The layers the pin holds; `None` follows the selection.
    pinned: Option<Vec<NodeId>>,
    /// Where each pinned layer sat when last seen, its parent and index, and the
    /// siblings it had there: a verb that swaps a layer for a new one (*Outline
    /// shape*, *Flatten*, *Convert to path*) selects what it made in that place,
    /// alone, and the pin moves to it. A sibling that slid into the place, or one
    /// of several layers selected (*Ungroup*'s children), is no heir. Kept only
    /// while the card shows.
    slots: Vec<(NodeId, (NodeId, usize), Vec<NodeId>)>,
    /// Whether the ⋯ menu is open.
    menu: bool,
    job: Option<Job<Landed>>,
    /// The document and its resolution as the worker last took them, by
    /// revision: a live edit renders after every input, and a deep copy per
    /// render cost the UI thread milliseconds on a large page.
    taken: Option<(u64, std::sync::Arc<(Document, Resolved)>)>,
    /// The image store between renders.
    store: Option<ImageStore>,
    shown: Option<Shown>,
    /// Since when, in egui time, the tiles have been stale with no render landing.
    waiting: Option<f64>,
    /// Input arrived during a live edit since the last render started: the preview
    /// may have moved, which no part of [`Want`] sees (a colour scrub, a typed
    /// character). Without it a held-still drag would render for ever.
    stirred: bool,
    /// The card's rect last frame: its width for the status bar, its edge for the
    /// loupe.
    pub(crate) card: Option<egui::Rect>,
}

impl SizePreview {
    /// Pin `ids`, as the head's pin does (tests only).
    #[cfg(test)]
    pub(crate) fn pin_selection(&mut self, ids: &[NodeId]) {
        self.pinned = Some(ids.to_vec());
    }
}

/// A tile's column width for size `n`.
fn column_w(n: u32) -> f32 {
    n as f32 + TILE_AIR
}

/// The width of the tiles alone.
fn tiles_w(sizes: &[u32]) -> f32 {
    sizes.iter().map(|n| column_w(*n)).sum()
}

/// Where `id` sits: its parent and its index among the parent's children.
fn slot_of(doc: &Document, id: NodeId) -> Option<(NodeId, usize)> {
    let parent = doc.get(id)?.parent()?;
    let index = doc.get(parent)?.children().iter().position(|c| *c == id)?;
    Some((parent, index))
}

/// The sizes `n` is the layer's own: its box's longer side is `n` units.
fn own_size(b: Rect, n: u32) -> bool {
    (b.width().max(b.height()) - f64::from(n)).abs() < 0.01
}

/// `sizes` with `n` ticked or unticked; `None` when that is refused — a seventh
/// size, or the last one going out.
pub(crate) fn toggled(sizes: &[u32], n: u32) -> Option<Vec<u32>> {
    let mut next = sizes.to_vec();
    match next.iter().position(|s| *s == n) {
        Some(_) if next.len() == 1 => return None,
        Some(at) => {
            next.remove(at);
        }
        None if next.len() >= MAX_SIZES => return None,
        None => {
            next.push(n);
            next.sort_unstable();
        }
    }
    Some(next)
}

/// Every tile of `want` drawn, on the worker. A hidden subject is drawn anyway, on
/// a copy shown, so a frame can be checked without its guide layers. A hidden
/// parent needs nothing: a subtree draws from its own layer down.
fn render(
    doc: &Document,
    res: &Resolved,
    overrides: &RenderOverrides,
    store: &mut ImageStore,
    want: &Want,
) -> Vec<Vec<(Vec<u8>, u32)>> {
    let hidden: Vec<Operation> = want
        .rows
        .iter()
        .filter(|(id, _)| doc.get(*id).is_some_and(|n| !n.visible()))
        .map(|(id, _)| Operation::SetVisible {
            id: *id,
            visible: true,
        })
        .collect();
    let shown = (!hidden.is_empty()).then(|| {
        let mut copy = doc.clone();
        let _ = copy.apply(&Transaction(hidden));
        // This thread's text engine holds Inter alone until it is given the
        // document's faces.
        ondin_core::text::register_font_data(&res.text_fonts());
        let res = Resolved::rebuild(&copy);
        (copy, res)
    });
    let (doc, res) = match &shown {
        Some((d, r)) => (d, r),
        None => (doc, res),
    };
    store.prepare(doc);
    let ppp = f32::from_bits(want.ppp);
    let mut renderer = VelloCpuRenderer::new();
    want.rows
        .iter()
        .map(|(id, view)| {
            let square = ondin_export::icon::square_of(*view);
            want.sizes
                .iter()
                .map(|&n| {
                    let px = match want.pixels {
                        true => n,
                        false => (n as f32 * ppp).round().max(1.0) as u32,
                    };
                    let vp = Viewport {
                        view: square,
                        pixel_size: (px, px),
                    };
                    let (rgba, _, _) =
                        renderer.render_subtrees_with(doc, res, &vp, store, &[*id], overrides);
                    (rgba, px)
                })
                .collect()
        })
        .collect()
}

impl OndinApp {
    /// How far right the status bar moves to clear the size preview: nothing while
    /// it is off.
    pub(crate) fn size_preview_clearance(&self) -> Option<f32> {
        match (self.show_size_preview, self.size_preview.card) {
            (true, Some(card)) if self.canvas_rect.is_positive() => Some(card.width() + GAP),
            (true, None) if self.canvas_rect.is_positive() => Some(MIN_INNER_W + PAD * 2.0 + GAP),
            _ => None,
        }
    }

    /// The layers the card shows: the pinned ones still in the document, or the
    /// selection, in the layers panel's order, top first. A pinned layer that is
    /// gone passes the pin to a selected layer in its place, else drops out; a
    /// pin whose layers are all gone lets go.
    fn size_preview_subjects(&mut self) -> Vec<NodeId> {
        let doc = &self.session.doc;
        let selected = self.session.selection.ids();
        let sp = &mut self.size_preview;
        if let Some(pinned) = &mut sp.pinned {
            for id in pinned.iter_mut() {
                if doc.get(*id).is_some() {
                    continue;
                }
                let heir = match selected {
                    [one] => sp
                        .slots
                        .iter()
                        .find(|(was, _, _)| was == id)
                        .filter(|(_, slot, siblings)| {
                            slot_of(doc, *one) == Some(*slot) && !siblings.contains(one)
                        })
                        .map(|_| one),
                    _ => None,
                };
                if let Some(heir) = heir {
                    *id = *heir;
                }
            }
            let mut kept: Vec<NodeId> = Vec::with_capacity(pinned.len());
            pinned.retain(|id| {
                let keep = doc.get(*id).is_some() && !kept.contains(id);
                kept.push(*id);
                keep
            });
            if pinned.is_empty() {
                sp.pinned = None;
            }
        }
        sp.slots = sp
            .pinned
            .iter()
            .flatten()
            .filter_map(|id| {
                let slot = slot_of(doc, *id)?;
                let siblings = doc.get(slot.0)?.children().to_vec();
                Some((*id, slot, siblings))
            })
            .collect();
        let ids = match &self.size_preview.pinned {
            Some(p) => p.clone(),
            None => self.session.selection.ids().to_vec(),
        };
        let mut ordered = ondin_core::build::in_document_order(doc, &ids);
        ordered.reverse();
        ordered
    }

    /// Land a finished render and start the next one when what is shown is stale.
    /// A live edit renders again after each input it gets, the newest landed shown.
    fn size_preview_pump(&mut self, ctx: &egui::Context, want: Option<Want>, live: bool) {
        let sp = &mut self.size_preview;
        let input = ctx.input(|i| !i.events.is_empty() || i.pointer.delta() != egui::Vec2::ZERO);
        sp.stirred |= live && input;
        if let Some(job) = &sp.job {
            match job.poll() {
                Ok(None) => {}
                Ok(Some(landed)) => {
                    sp.job = None;
                    sp.store = Some(landed.store);
                    // The patience counts from the last landing, so a drag whose
                    // renders keep up never fades; a slow one does.
                    sp.waiting = None;
                    let filter = match landed.want.pixels {
                        true => egui::TextureOptions::NEAREST,
                        false => egui::TextureOptions::LINEAR,
                    };
                    let textures = landed
                        .tiles
                        .into_iter()
                        .enumerate()
                        .map(|(r, row)| {
                            row.into_iter()
                                .enumerate()
                                .map(|(c, (rgba, px))| {
                                    let side = px as usize;
                                    let image = egui::ColorImage::from_rgba_unmultiplied(
                                        [side, side],
                                        &rgba,
                                    );
                                    ctx.load_texture(format!("size-preview-{r}-{c}"), image, filter)
                                })
                                .collect()
                        })
                        .collect();
                    sp.shown = Some(Shown {
                        want: landed.want,
                        textures,
                    });
                }
                Err(()) => sp.job = None,
            }
        }
        let Some(want) = want else {
            sp.waiting = None;
            sp.taken = None;
            return;
        };
        let current = sp.shown.as_ref().is_some_and(|s| s.want == want);
        if current && !sp.stirred {
            sp.waiting = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        if !current {
            sp.waiting.get_or_insert(now);
        }
        if sp.job.is_some() {
            return;
        }
        let rev = self.session.revision();
        let (doc, res, overrides) = self.session.render_inputs();
        let taken = match &sp.taken {
            Some((r, both)) if *r == rev => both.clone(),
            _ => {
                let both = std::sync::Arc::new((doc.clone(), res.clone()));
                sp.taken = Some((rev, both.clone()));
                both
            }
        };
        // The edit in flight, without what only explains a mode on the canvas.
        let mut overrides = overrides.clone();
        overrides.set_outline(None);
        overrides.set_chrome_ghost(None);
        sp.stirred = false;
        let mut store = sp.store.take().unwrap_or_default();
        sp.job = Some(Job::spawn("ondin-size-preview", ctx, move || {
            let (doc, res) = &*taken;
            let tiles = render(doc, res, &overrides, &mut store, &want);
            Landed { want, tiles, store }
        }));
    }

    /// Draw the size preview and act on its input. Placed from `canvas_rect`, which
    /// is last frame's when this runs before the canvas, as the minimap is.
    pub(crate) fn size_preview_ui(&mut self, ctx: &egui::Context) {
        let canvas = self.canvas_rect;
        if !self.show_size_preview || !canvas.is_positive() {
            self.size_preview.menu = false;
            self.size_preview.card = None;
            // What changes while hidden is not seen happen, so no heir is read
            // from it.
            self.size_preview.slots.clear();
            return;
        }
        let prefs = self.prefs.size_preview.clone();
        let subjects = self.size_preview_subjects();
        // A layer with no box gets no row, so it is not counted either.
        let mut rows: Vec<(NodeId, Rect)> = subjects
            .iter()
            .filter_map(|id| Some((*id, self.session.preview_world_bounds(*id)?)))
            .collect();
        let total = rows.len();
        rows.truncate(MAX_ROWS);
        let live = !self.session.render_inputs().2.is_empty();
        let ppp = ctx.pixels_per_point();
        let want = (!rows.is_empty()).then(|| Want {
            rows: rows.clone(),
            sizes: prefs.sizes.clone(),
            pixels: prefs.pixels,
            ppp: ppp.to_bits(),
            revision: self.session.revision(),
        });
        self.size_preview_pump(ctx, want, live && !rows.is_empty());
        let now = ctx.input(|i| i.time);
        let slow = self
            .size_preview
            .waiting
            .is_some_and(|t| now - t > crate::cleanup::FIT_PATIENCE);
        if slow || self.size_preview.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(1.0 / 30.0));
        }

        let several = rows.len() > 1;
        // Both grounds stack for one layer; several show one at a time, light
        // when the setting is both.
        let grounds: Vec<egui::Color32> = match (prefs.ground, several) {
            (SizeGround::Light, _) | (SizeGround::Both, true) => vec![color::light_ground()],
            (SizeGround::Dark, _) => vec![color::dark_ground()],
            (SizeGround::Both, false) => vec![color::light_ground(), color::dark_ground()],
        };
        let inner_w = tiles_w(&prefs.sizes).max(MIN_INNER_W);
        let mut next = prefs.clone();
        let mut pin = None;
        let mut hovered_tile: Option<(usize, usize, egui::Rect)> = None;
        let mut menu_button = None;

        let area = egui::Area::new(egui::Id::new("size-preview"))
            .order(egui::Order::Middle)
            .fade_in(false)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(canvas.left_bottom() + egui::vec2(ui::SCREEN_MARGIN, -ui::SCREEN_MARGIN))
            .show(ctx, |ui| {
                ui::menu_frame(PAD).show(ui, |ui| {
                    ui.set_width(inner_w);
                    ui.spacing_mut().item_spacing = egui::vec2(PAD, PAD);
                    // --- head --------------------------------------------------
                    let (head, _) =
                        ui.allocate_exact_size(egui::vec2(inner_w, HEAD_H), egui::Sense::empty());
                    let mut head_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(head)
                            .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    );
                    head_ui.spacing_mut().item_spacing.x = 4.0;
                    // The comparison bar's controls: a toolbar-height segmented
                    // track, and grounded toggles as tall as it.
                    let more = ui::field_button(
                        &mut head_ui,
                        icon::DOTS_THREE,
                        ui::TOOLBAR_H,
                        ui::FieldButton::on_if(self.size_preview.menu),
                    )
                    .on_hover_text("Sizes");
                    if more.clicked() {
                        self.size_preview.menu = !self.size_preview.menu;
                    }
                    menu_button = Some(more.rect);
                    let pinned = self.size_preview.pinned.is_some();
                    let pin_state = match (pinned, rows.is_empty()) {
                        (true, _) => ui::FieldButton::On,
                        (false, empty) => ui::FieldButton::enabled_if(!empty),
                    };
                    let pin_button =
                        ui::field_button(&mut head_ui, icon::PUSH_PIN, ui::TOOLBAR_H, pin_state)
                            .on_hover_text(match pinned {
                                true => "Follow the selection again",
                                false => "Keep showing these layers",
                            });
                    if pin_button.clicked() && pin_state.is_enabled() {
                        pin = Some(!pinned);
                    }
                    let px = ui::field_button(
                        &mut head_ui,
                        icon::PIXEL_PREVIEW,
                        ui::TOOLBAR_H,
                        ui::FieldButton::on_if(prefs.pixels),
                    )
                    .on_hover_text(match prefs.pixels {
                        true => "Show the vector at this screen's density",
                        false => "Show the pixels a 1× screen gets",
                    });
                    if px.clicked() {
                        next.pixels = !prefs.pixels;
                    }
                    // Several layers show one ground at a time, so *Both* goes.
                    let cells = if several { 2 } else { 3 };
                    let lit = match (prefs.ground, several) {
                        (SizeGround::Light, _) | (SizeGround::Both, true) => 0,
                        (SizeGround::Dark, _) => 1,
                        (SizeGround::Both, false) => 2,
                    };
                    let tips = ["Light ground", "Dark ground", "Both grounds"];
                    if let Some(i) = ui::segmented_tipped(
                        &mut head_ui,
                        // Each cell a toolbar button wide, inside the track's inset.
                        cells as f32 * ui::TOOLBAR_H + 4.0,
                        ui::TOOLBAR_CELL_H,
                        cells,
                        lit,
                        |_| true,
                        |i| tips[i],
                        |p, i, r, _| paint_ground_cell(p, i, r),
                    ) {
                        next.ground = [SizeGround::Light, SizeGround::Dark, SizeGround::Both][i];
                    }
                    // The subject, left of the controls, cut to the room they leave;
                    // glyph and name set as a menu row's.
                    let room = head_ui.available_rect_before_wrap();
                    let p = ui.painter().with_clip_rect(room);
                    let ink = theme::text::muted();
                    let glyph_pt = ui::TOOLBAR_GLYPH_PT;
                    let glyph_at = egui::pos2(room.left() + glyph_pt / 2.0, room.center().y);
                    if slow {
                        ui::paint_spinner(&p, glyph_at, glyph_pt, ink, now);
                    } else {
                        p.text(
                            glyph_at,
                            egui::Align2::CENTER_CENTER,
                            icon::SIZE_PREVIEW,
                            theme::icon_font(glyph_pt),
                            ink,
                        );
                    }
                    let x = room.left() + glyph_pt + 6.0;
                    let name = match rows.as_slice() {
                        [] => String::new(),
                        [(id, _)] => crate::private::layer(&self.session.doc, *id).into_owned(),
                        _ => ui::count(total, "layer"),
                    };
                    let name_g = ui.ctx().fonts_mut(|f| {
                        let mut job = egui::text::LayoutJob::simple_singleline(
                            name,
                            egui::FontId::proportional(ui::SMALL_PT),
                            ink,
                        );
                        job.wrap = egui::text::TextWrapping::truncate_at_width(
                            (room.right() - x).max(0.0),
                        );
                        f.layout_job(job)
                    });
                    let y = room.center().y - name_g.size().y / 2.0;
                    p.galley(egui::pos2(x, y), name_g, ink);

                    // --- the tiles ---------------------------------------------
                    if rows.is_empty() {
                        ui::empty_note(ui, "Select layers to preview them at size");
                        return;
                    }
                    let shown = self.size_preview.shown.as_ref();
                    let fade = if slow { 0.5 } else { 1.0 };
                    let tallest = prefs.sizes.iter().copied().max().unwrap_or(16) as f32;
                    let strip_h = tallest + STRIP_AIR;
                    for (r, (id, _)) in rows.iter().enumerate() {
                        for ground in &grounds {
                            let (line, _) = ui.allocate_exact_size(
                                egui::vec2(inner_w, strip_h),
                                egui::Sense::empty(),
                            );
                            let p = ui.painter();
                            let strip = egui::Rect::from_min_size(
                                line.min,
                                egui::vec2(tiles_w(&prefs.sizes), strip_h),
                            );
                            // No hairline: on the dark ground it reads as an inset,
                            // and the dark strip looks narrower than the light.
                            p.rect_filled(strip, egui::CornerRadius::same(4), *ground);
                            let mut x = strip.left();
                            for (c, n) in prefs.sizes.iter().enumerate() {
                                let cell = egui::Rect::from_min_size(
                                    egui::pos2(x, strip.top()),
                                    egui::vec2(column_w(*n), strip_h),
                                );
                                x += column_w(*n);
                                let tex = shown
                                    .filter(|s| {
                                        s.want.sizes == prefs.sizes
                                            && s.want.rows.get(r).is_some_and(|(i, _)| i == id)
                                    })
                                    .and_then(|s| s.textures.get(r)?.get(c));
                                let Some(tex) = tex else { continue };
                                let side = *n as f32;
                                let min = cell.center() - egui::vec2(side, side) / 2.0;
                                let min = egui::pos2(
                                    (min.x * ppp).round() / ppp,
                                    (min.y * ppp).round() / ppp,
                                );
                                let at = egui::Rect::from_min_size(min, egui::vec2(side, side));
                                p.image(
                                    tex.id(),
                                    at,
                                    crate::thumbs::FULL_UV,
                                    egui::Color32::WHITE.gamma_multiply(fade),
                                );
                                if prefs.pixels && ui.rect_contains_pointer(cell) {
                                    p.rect_stroke(
                                        at.expand(1.0),
                                        egui::CornerRadius::ZERO,
                                        egui::Stroke::new(1.5, color::SELECT),
                                        egui::StrokeKind::Outside,
                                    );
                                    hovered_tile = Some((r, c, at));
                                }
                            }
                        }
                    }
                    // --- the size captions, a dot where every row is its own size
                    let (caps, _) =
                        ui.allocate_exact_size(egui::vec2(inner_w, CAPS_H), egui::Sense::empty());
                    let p = ui.painter();
                    let mut x = caps.left();
                    for n in &prefs.sizes {
                        let mid = egui::pos2(x + column_w(*n) / 2.0, caps.center().y);
                        x += column_w(*n);
                        let own = rows.iter().all(|(_, b)| own_size(*b, *n));
                        let ink = if own {
                            theme::text::muted()
                        } else {
                            theme::text::faint()
                        };
                        let g = p.layout_no_wrap(
                            n.to_string(),
                            egui::FontId::proportional(ui::CAPTION_PT),
                            ink,
                        );
                        let w = g.size().x + if own { 6.0 } else { 0.0 };
                        p.galley(
                            egui::pos2(mid.x - w / 2.0, mid.y - g.size().y / 2.0),
                            g,
                            ink,
                        );
                        if own {
                            p.circle_filled(
                                egui::pos2(mid.x + w / 2.0 - 1.5, mid.y),
                                1.5,
                                color::accent(),
                            );
                        }
                    }
                    if total > MAX_ROWS {
                        ui.label(
                            egui::RichText::new(format!("Showing {MAX_ROWS} of {total} layers"))
                                .size(ui::CAPTION_PT)
                                .color(theme::text::faint()),
                        );
                    }
                });
            });
        self.size_preview.card = Some(area.response.rect);

        if let Some(on) = pin {
            self.size_preview.pinned = on.then(|| subjects.clone());
        }
        let card = area.response.rect;
        if let Some(button) = menu_button
            && self.size_preview.menu
        {
            self.size_preview_menu(ctx, card, button, &mut next);
        }
        // The menu and the loupe both hang off the card's right edge.
        if let Some((r, c, _)) = hovered_tile
            && !self.size_preview.menu
        {
            self.size_preview_loupe(ctx, card, r, c);
        }
        if next != prefs {
            self.prefs.size_preview = next;
            self.prefs.save();
        }
    }

    /// The ⋯ menu: the sizes, ten ticks, six at most and one at least. It opens
    /// beside the card, level with the ⋯, so the ⋯ stays clear to close it.
    fn size_preview_menu(
        &mut self,
        ctx: &egui::Context,
        card: egui::Rect,
        button: egui::Rect,
        next: &mut SizePreviewPrefs,
    ) {
        let id = egui::Id::new("size-preview-menu");
        let labels: Vec<String> = SIZE_CHOICES.iter().map(u32::to_string).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let w = ui::menu_check_width(ctx, &refs, ui::MENU_PAD, MENU_W);
        let beside = egui::Rect::from_x_y_ranges(card.x_range(), button.y_range());
        let menu = ui::flyout(id, beside, w)
            .order(egui::Order::Foreground)
            .show(ctx, (SIZE_CHOICES.len(), 0), |ui| {
                for (n, label) in SIZE_CHOICES.iter().zip(&labels) {
                    let on = next.sizes.contains(n);
                    let allowed = toggled(&next.sizes, *n);
                    let row = ui::MenuRow::new("", label)
                        .checked(on)
                        .enabled(allowed.is_some());
                    let resp = ui::menu_row(ui, row, ui::MENU_ROW_H);
                    let resp = match (allowed.is_some(), on) {
                        (true, _) => resp,
                        (false, true) => resp.on_hover_text("One size at least"),
                        (false, false) => resp.on_hover_text("Six sizes at most"),
                    };
                    if resp.clicked()
                        && let Some(sizes) = allowed
                    {
                        next.sizes = sizes;
                    }
                }
            });
        let away = ctx.input(|i| i.pointer.any_click())
            && !menu.contains_pointer()
            && !ctx
                .pointer_interact_pos()
                .is_some_and(|p| button.contains(p));
        if away || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.size_preview.menu = false;
        }
        // Heard, so the Escape that closes it spends no rung of the ladder.
        if self.size_preview.menu {
            self.popover_heard = true;
        }
    }

    /// Whether the ⋯ menu is open and so claims an `Escape` press.
    pub(crate) fn size_preview_owns_escape(&self) -> bool {
        self.size_preview.menu
    }

    /// The loupe beside the card: the hovered pixel tile at [`LOUPE_ZOOM`], each
    /// pixel a block with a hairline between, never resampled.
    fn size_preview_loupe(&self, ctx: &egui::Context, card: egui::Rect, r: usize, c: usize) {
        let Some(tex) = self
            .size_preview
            .shown
            .as_ref()
            .and_then(|s| s.textures.get(r)?.get(c))
        else {
            return;
        };
        let [w, _] = tex.size();
        let zoom = LOUPE_ZOOM;
        let side = w as f32 * zoom;
        egui::Area::new(egui::Id::new("size-preview-loupe"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(card.right_bottom() + egui::vec2(GAP, 0.0))
            .show(ctx, |ui| {
                ui::menu_frame(PAD + 2.0).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = PAD;
                    ui.label(
                        egui::RichText::new(format!("{w} × {w} · ×{}", zoom as u32))
                            .size(ui::SMALL_PT)
                            .color(theme::text::dim()),
                    );
                    let (at, _) =
                        ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::empty());
                    let p = ui.painter();
                    ui::paint_checkerboard(p, at, 5.0);
                    p.image(tex.id(), at, crate::thumbs::FULL_UV, egui::Color32::WHITE);
                    let line = egui::Stroke::new(1.0, color::text_a(28));
                    for i in 1..w {
                        let o = i as f32 * zoom;
                        p.line_segment(
                            [
                                egui::pos2(at.left() + o, at.top()),
                                egui::pos2(at.left() + o, at.bottom()),
                            ],
                            line,
                        );
                        p.line_segment(
                            [
                                egui::pos2(at.left(), at.top() + o),
                                egui::pos2(at.right(), at.top() + o),
                            ],
                            line,
                        );
                    }
                });
            });
    }
}

/// A grounds cell: the light ground, the dark one, or the two halved, as a chip
/// rather than a glyph, since the grounds are colours.
fn paint_ground_cell(p: &egui::Painter, i: usize, r: egui::Rect) {
    let chip = egui::Rect::from_center_size(r.center(), egui::vec2(10.0, 10.0));
    match i {
        0 => ui::paint_colour_chip(p, chip, color::light_ground()),
        1 => ui::paint_colour_chip(p, chip, color::dark_ground()),
        _ => ui::paint_colour_chip_halved(p, chip, color::light_ground(), color::dark_ground()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repeat::tests::pass;

    /// Wait for the render in flight and leave it for the next frame to land.
    fn settle(ctx: &egui::Context, app: &mut OndinApp) {
        for _ in 0..3 {
            if let Some(job) = app.size_preview.job.take()
                && let Some(landed) = job.wait()
            {
                app.size_preview.job = Some(Job::done(landed));
            }
            pass(ctx, app, Vec::new());
        }
    }

    /// An app with the preview on and the starter frame selected; answers the frame.
    fn app_with_preview(ctx: &egui::Context) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let page = app.session.doc.first_page();
        let frame = app.session.doc.get(page).expect("the page").children()[0];
        app.session.selection.set_one(frame);
        app.show_size_preview = true;
        app.show_minimap = true;
        pass(ctx, &mut app, Vec::new());
        settle(ctx, &mut app);
        (app, frame)
    }

    /// The tiles' sides in pixels, per row.
    fn sides(app: &OndinApp) -> Vec<Vec<usize>> {
        let shown = app.size_preview.shown.as_ref().expect("a render landed");
        shown
            .textures
            .iter()
            .map(|row| row.iter().map(|t| t.size()[0]).collect())
            .collect()
    }

    /// One selected layer lands a tile per size at the display's density, the
    /// minimap steps aside while the card holds its corner, and the status bar
    /// clears the card.
    /// flip: `minimap_ui` drawing while the size preview shows → fails at "the
    /// minimap is hidden".
    #[test]
    fn a_layer_renders_at_each_size_in_the_minimaps_corner() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_preview(&ctx);
        assert_eq!(sides(&app), [vec![16, 20, 24, 32, 48]]);
        pass(&ctx, &mut app, Vec::new());
        assert!(
            app.size_preview.job.is_none(),
            "nothing changed, nothing rendering"
        );
        assert!(
            ctx.memory(|m| m.area_rect(egui::Id::new("minimap")))
                .is_none()
                || !ctx.memory(|m| m.areas().visible_last_frame(&egui::LayerId::new(
                    egui::Order::Middle,
                    egui::Id::new("minimap")
                ))),
            "the minimap is hidden"
        );
        let card = app.size_preview.card.expect("the card is up");
        assert_eq!(app.minimap_clearance(), card.width() + GAP);
        assert!(app.show_minimap, "hidden, not switched off");
    }

    /// With pixels on, a tile is exactly N pixels on a 2× display; off, it is the
    /// display's 2N.
    /// flip: `render` ignoring `want.pixels` → fails at the pixels-on sides.
    #[test]
    fn pixels_on_renders_each_size_at_its_own_pixels() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.0);
        let (mut app, _) = app_with_preview(&ctx);
        settle(&ctx, &mut app);
        assert_eq!(sides(&app), [vec![32, 40, 48, 64, 96]], "pixels off");
        app.prefs.size_preview.pixels = true;
        pass(&ctx, &mut app, Vec::new());
        settle(&ctx, &mut app);
        assert_eq!(sides(&app), [vec![16, 20, 24, 32, 48]], "pixels on");
    }

    /// Pinned, the card keeps its layer when the selection moves on; once the
    /// layer is deleted the pin lets go and the card follows the selection.
    #[test]
    fn the_pin_holds_its_layers_until_they_are_gone() {
        let ctx = egui::Context::default();
        let (mut app, frame) = app_with_preview(&ctx);
        app.size_preview.pinned = Some(vec![frame]);
        app.session.selection.clear();
        assert_eq!(app.size_preview_subjects(), [frame]);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: frame }]))
        );
        assert!(app.size_preview_subjects().is_empty());
        assert!(app.size_preview.pinned.is_none(), "the pin let go");
    }

    /// A layer in a hidden group is drawn as it is, with nothing shown on a copy:
    /// its subtree draws from the layer down.
    #[test]
    fn a_layer_in_a_hidden_group_is_drawn() {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::{Brush, Fill, IdSource, NodeKind, keyed_by_position};
        let mut ids = IdSource::new(0x5A0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (group, dot) = (ids.mint(), ids.mint());
        let ops = vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: dot,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(8.0, 8.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetFills {
                id: dot,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(ondin_core::peniko::Color::BLACK).into(),
                    visible: true,
                }]),
            },
            Operation::SetVisible {
                id: group,
                visible: false,
            },
        ];
        doc.apply(&Transaction(ops)).expect("the fixture");
        let res = Resolved::rebuild(&doc);
        let want = Want {
            rows: vec![(dot, Rect::new(0.0, 0.0, 8.0, 8.0))],
            sizes: vec![8],
            pixels: true,
            ppp: 1.0f32.to_bits(),
            revision: 0,
        };
        let tiles = render(
            &doc,
            &res,
            &RenderOverrides::default(),
            &mut ImageStore::new(),
            &want,
        );
        let (rgba, px) = &tiles[0][0];
        assert_eq!(*px, 8);
        let centre = ((4 * 8 + 4) * 4 + 3) as usize;
        assert_eq!(rgba[centre], 255, "the dot is drawn");
    }

    /// Sizes tick in order, a seventh and the last one going out are refused.
    #[test]
    fn the_sizes_hold_one_to_six() {
        assert_eq!(toggled(&[16, 48], 24), Some(vec![16, 24, 48]));
        assert_eq!(toggled(&[16, 24], 16), Some(vec![24]));
        assert_eq!(toggled(&[16], 16), None, "one at least");
        assert_eq!(toggled(&[12, 16, 20, 24, 32, 48], 64), None, "six at most");
    }

    /// Escape closes the ⋯ menu and only that: the selection stays.
    /// flip: `a_popover_owns_escape` without the size preview's menu → fails at
    /// "the selection stays".
    #[test]
    fn escape_closes_the_sizes_menu_and_keeps_the_selection() {
        let ctx = egui::Context::default();
        let (mut app, frame) = app_with_preview(&ctx);
        app.size_preview.menu = true;
        pass(&ctx, &mut app, Vec::new());
        assert!(app.size_preview.menu, "the fixture opened the menu");
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        pass(&ctx, &mut app, vec![escape]);
        assert!(!app.size_preview.menu, "the menu closed");
        assert_eq!(app.session.selection.ids(), [frame], "the selection stays");
    }

    /// *Outline shape* on a pinned rectangle swaps it for a new path in its place,
    /// selected; the pin moves to the path rather than letting go.
    /// flip: `size_preview_subjects` skipping the heir lookup → fails at "the pin
    /// moved to the path".
    #[test]
    fn the_pin_follows_a_layer_swapped_in_its_place() {
        use ondin_core::NodeKind;
        use ondin_core::kurbo::{Affine, Size};
        let ctx = egui::Context::default();
        let (mut app, frame) = app_with_preview(&ctx);
        let rect = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(16.0, 16.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }])));
        app.session.selection.set_one(rect);
        app.size_preview.pin_selection(&[rect]);
        assert_eq!(app.size_preview_subjects(), [rect], "the fixture pinned it");
        app.outline_shape(rect);
        assert!(
            app.session.doc.get(rect).is_none(),
            "the rectangle was replaced"
        );
        let path = app.session.selection.ids()[0];
        assert_eq!(
            app.size_preview_subjects(),
            [path],
            "the pin moved to the path"
        );
        assert!(app.size_preview.pinned.is_some());
        app.session.selection.clear();
        assert_eq!(app.size_preview_subjects(), [path], "and holds it");

        // A sibling that slides into a deleted pinned layer's place is no heir,
        // though selected alone: the pin lets go.
        // flip: the heir test without its `siblings` check → fails at "lets go".
        let below = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id: below,
            parent: frame,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(8.0, 8.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }])));
        assert_eq!(app.size_preview_subjects(), [path]);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::DeleteNode { id: path }]))
        );
        app.session.selection.set_one(below);
        app.size_preview_subjects();
        assert!(app.size_preview.pinned.is_none(), "lets go");
    }
}
