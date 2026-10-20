//! The eyedropper (design canvas, *Eyedropper*): picks a colour off the rendered
//! canvas — onto the selection from the rail (`I`), or into one slot from a
//! colour picker's pick button.
//!
//! A click asks for the sample under it and the write waits for that sample
//! ([`PendingPick`]), because the GPU readback lands a frame later
//! (`canvas::CanvasRenderer::request_sample`). Nothing is previewed on the canvas:
//! a preview would change the very pixels the next sample reads (the design's
//! rule 4).

use crate::app::OndinApp;
use crate::canvas::{LOUPE_SIDE, Sample, SampleKey};
use crate::panels::paint;
use crate::tools::Tool;
use eframe::egui;
use ondin_core::Brush;
use ondin_core::peniko::Color;

/// Where an armed eyedropper's pick lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DropTarget {
    /// The rail's: each selected layer's top fill, or stroke with Alt.
    Selection,
    /// A picker's pick button: that picker's slot.
    Picker,
}

/// A click waiting for the sample under it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct PendingPick {
    /// What the click was over; the write needs a sample with this key.
    pub key: SampleKey,
    /// Alt: the top stroke rather than the top fill.
    pub stroke: bool,
    /// Shift: stay armed after the pick.
    pub stay: bool,
}

/// The loupe's cells, in points.
const CELL: f32 = 8.0;
/// The gap between the loupe and its hex chip.
const CHIP_GAP: f32 = 5.0;
const CHIP_H: f32 = 20.0;
/// Where the "Off canvas" hint's top-centre sits from the pointer: under the
/// dropper cursor's glyph, which hangs down and right of its tip.
const OFF_CANVAS_HINT: egui::Vec2 = egui::vec2(10.0, 24.0);

impl OndinApp {
    /// What an armed eyedropper would pick into, or `None` when it is not armed.
    /// A picker's pick button outranks the rail tool: it is the narrower ask.
    pub(crate) fn dropper_armed(&self) -> Option<DropTarget> {
        if self.picker.as_ref().is_some_and(|p| p.picking) {
            Some(DropTarget::Picker)
        } else if self.tool == Tool::Eyedropper {
            Some(DropTarget::Selection)
        } else {
            None
        }
    }

    /// Whether the canvas answers as a selecting tool: the tool selects and no
    /// eyedropper is armed. A picker's pick button arms one with the tool left on
    /// Select, so `Tool::selects` alone would still outline what a hover is over.
    pub(crate) fn selecting(&self) -> bool {
        self.tool.selects() && self.dropper_armed().is_none()
    }

    /// Disarm whichever eyedropper is armed, writing nothing: the picker's pick
    /// button first, then the rail tool, which hands back to the tool it was armed
    /// from. False when neither was.
    pub(crate) fn disarm_dropper(&mut self) -> bool {
        self.dropper_pick = None;
        if let Some(p) = self.picker.as_mut().filter(|p| p.picking) {
            p.picking = false;
            p.preview = None;
            return true;
        }
        if self.tool == Tool::Eyedropper {
            self.tool = self.dropper_return;
            return true;
        }
        false
    }

    /// While an eyedropper is armed, egui sees the pointer only where a click
    /// picks: the canvas, and the picker when it is the one picking. Elsewhere it
    /// is told the pointer is gone and gets no press and no wheel, so no panel
    /// hovers, takes a click or scrolls; [`OndinApp::dropper_pointer`] keeps the
    /// position for the dropper's own cursor. Run on the raw input, before egui.
    ///
    /// A press egui already has finishes untouched wherever it goes, so a pan
    /// dragged off the canvas still ends — and a release with no press egui saw
    /// is dropped, which is what spends the release of a swallowed press.
    pub(crate) fn gate_dropper_input(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        let Some(target) = self.dropper_armed() else {
            // Disarmed with the pointer hidden: hand it back where it is, or the
            // panel under it would not hover until it moved.
            if let Some(p) = self.dropper_pointer.take() {
                raw.events.insert(0, egui::Event::PointerMoved(p));
            }
            return;
        };
        let (canvas, picker) = (self.canvas_rect, target == DropTarget::Picker);
        // The panels all draw in the background layer, and the canvas is the one
        // among them that picks; any area above it is chrome.
        let live = |p: egui::Pos2| match ctx.layer_id_at(p) {
            Some(l) if l.id == crate::panels::picker_layer_id() => picker,
            Some(l) if l.order != egui::Order::Background => false,
            _ => canvas.contains(p),
        };
        let (mut down, latest) = ctx.input(|i| (i.pointer.any_down(), i.pointer.latest_pos()));
        let mut hidden = self.dropper_pointer;
        // Armed with the pointer resting on a panel — `I`, or the rail's own
        // button: hide it now rather than at its first move.
        if hidden.is_none()
            && !down
            && let Some(p) = latest.filter(|p| !live(*p))
        {
            hidden = Some(p);
            raw.events.insert(0, egui::Event::PointerGone);
        }
        let mut at = hidden.or(latest);
        raw.events.retain_mut(|e| match *e {
            egui::Event::PointerMoved(p) => {
                at = Some(p);
                if down || live(p) {
                    hidden = None;
                } else {
                    hidden = Some(p);
                    *e = egui::Event::PointerGone;
                }
                true
            }
            egui::Event::PointerGone => {
                (at, hidden) = (None, None);
                true
            }
            egui::Event::PointerButton { pos, pressed, .. } => {
                let keep = if pressed { down || live(pos) } else { down };
                if keep {
                    down = pressed;
                }
                keep
            }
            egui::Event::MouseWheel { .. } | egui::Event::Zoom(_) | egui::Event::Rotate(_) => {
                down || at.is_none_or(live)
            }
            _ => true,
        });
        self.dropper_pointer = hidden;
    }

    /// The canvas's input while an eyedropper is armed: a click asks for the
    /// pixel under it, and nothing else on the canvas answers the press. A
    /// right-click is spent by `cancel_gesture`'s caller before this runs.
    pub(crate) fn dropper_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        if !resp.clicked() {
            return;
        }
        let Some(p) = resp.interact_pointer_pos() else {
            return;
        };
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        self.dropper_picks += 1;
        self.dropper_pick = Some(PendingPick {
            key: SampleKey {
                pick: self.dropper_picks,
                ..self.sample_key(p, rect, ppp)
            },
            stroke: alt,
            stay: shift,
        });
    }

    /// The sample key for the device pixel under `p`, against what the canvas is
    /// showing now.
    pub(crate) fn sample_key(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> SampleKey {
        let (w, h) = self.canvas_px;
        let px = |v: f32, max: u32| ((v * ppp).floor().max(0.0) as u32).min(max.saturating_sub(1));
        SampleKey {
            at: (px(p.x - rect.min.x, w), px(p.y - rect.min.y, h)),
            revision: self.session.revision(),
            center: self.session.camera.center,
            zoom: self.session.camera.zoom,
            size: self.canvas_px,
            pick: 0,
        }
    }

    /// Land a pick whose sample has arrived: `rgb` is the pixel under the click.
    pub(crate) fn land_pick(&mut self, ctx: &egui::Context, pick: PendingPick, rgb: [u8; 3]) {
        let [r, g, b] = rgb;
        let color = Color::from_rgba8(r, g, b, 255);
        match self.dropper_armed() {
            Some(DropTarget::Picker) => {
                let Some((node, slot)) = self.picker.as_ref().map(|p| (p.node, p.slot)) else {
                    return;
                };
                self.write_slot_rgb(node, slot, color);
                if !pick.stay
                    && let Some(p) = &mut self.picker
                {
                    p.picking = false;
                    p.preview = None;
                }
            }
            Some(DropTarget::Selection) => {
                let ids = self.session.selection.ids().to_vec();
                if ids.is_empty() {
                    let hex = crate::ui::hex_of(color);
                    ctx.copy_text(hex.clone());
                    self.session
                        .info(format!("Copied {hex} · nothing selected"));
                } else {
                    let target = if pick.stroke {
                        ondin_core::PaintTarget::Stroke
                    } else {
                        ondin_core::PaintTarget::Fill
                    };
                    let tx = ondin_core::set_top_paint_all(
                        &self.session.doc,
                        &ids,
                        target,
                        |b| Brush::Solid(color.with_alpha(b.map_or(1.0, paint::alpha_of))),
                        &mut self.session.ids,
                    );
                    self.commit_edit(tx);
                }
                if !pick.stay {
                    self.disarm_dropper();
                }
            }
            None => {}
        }
    }

    /// Sample under the pointer, land a waiting pick, feed the picker's preview,
    /// and draw the loupe — once a frame, after the canvas has rendered.
    /// Returns true while a readback is in flight.
    pub(crate) fn dropper_frame(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        let Some(target) = self.dropper_armed() else {
            self.dropper_pick = None;
            return false;
        };
        let ctx = ui.ctx().clone();
        // Off the canvas egui is told the pointer is gone (`gate_dropper_input`).
        let pointer = ctx.pointer_hover_pos().or(self.dropper_pointer);
        let over_canvas = pointer
            .filter(|_| resp.hovered())
            .filter(|p| rect.contains(*p));
        let camera = self.session.camera;
        let backdrop = self.session.preview_canvas_background();
        let mut busy = false;
        // The pick first: it is the sample that must land, and the hover is
        // usually the same pixel.
        let want = self
            .dropper_pick
            .map(|p| p.key)
            .or_else(|| over_canvas.map(|p| self.sample_key(p, rect, ppp)));
        if let Some(key) = want {
            let (doc, res, _) = self.session.render_inputs();
            busy = self.canvas.request_sample(key, doc, res, &camera, backdrop);
        }
        if let Some(pick) = self.dropper_pick {
            let landed = self
                .canvas
                .sample()
                .filter(|s| s.key == pick.key)
                .map(|s| s.center());
            match landed {
                Some(Some(rgb)) => {
                    self.dropper_pick = None;
                    self.land_pick(&ctx, pick, rgb);
                }
                Some(None) => self.dropper_pick = None,
                None => {}
            }
        }
        // The picker shows the latest sample while the pointer is on the canvas,
        // as the loupe does; off it, its own colour.
        //
        // ⚠️ **The latest sample, not the one for this pixel.** Each pixel of
        // movement starts a readback that lands a frame later, and waiting for the
        // matching one put the slot's own colour back for that frame: the picker
        // flickered on every move. Headless samples land at once, so no test
        // here can see this.
        let hovered = over_canvas
            .and(self.canvas.sample())
            .and_then(Sample::center);
        if target == DropTarget::Picker
            && let Some(p) = &mut self.picker
        {
            p.preview = hovered;
        }
        if let Some(p) = pointer {
            let over_picker = ctx
                .layer_id_at(p)
                .is_some_and(|l| l.id == crate::panels::picker_layer_id());
            if over_canvas.is_some() {
                draw_loupe(&ctx, p, self.canvas.sample(), ctx.content_rect());
            } else if !over_picker {
                // Off the canvas the loupe gives way to the dropper cursor and a
                // hint under it (L5); the picker keeps its own pointer.
                ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
                ctx.set_cursor_image(self.cursors.image(&ctx, crate::cursor::Bitmap::Eyedropper));
                draw_chip(
                    &loupe_painter(&ctx),
                    p.x + OFF_CANVAS_HINT.x,
                    p.y + OFF_CANVAS_HINT.y,
                    None,
                );
            }
        }
        busy
    }
}

/// The painter the loupe and its chip draw with, over every panel and popover.
fn loupe_painter(ctx: &egui::Context) -> egui::Painter {
    ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("eyedropper-loupe"),
    ))
}

/// The loupe centred on the hotspot `at`, with its hex chip under it — above it
/// where the chip would leave `screen` (the design's L1, L2). Before the first
/// sample lands it is an empty frame.
///
/// **Two rims, dark outside light**, under a soft shadow: whatever the ground,
/// one of the pair contrasts with it, so the loupe never dissolves into a
/// flat colour the way a single light hairline did over a pale page.
fn draw_loupe(ctx: &egui::Context, at: egui::Pos2, sample: Option<&Sample>, screen: egui::Rect) {
    let painter = loupe_painter(ctx);
    let side = LOUPE_SIDE as f32 * CELL;
    let loupe = egui::Rect::from_center_size(at, egui::vec2(side, side));
    let radius = egui::CornerRadius::same(8);
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: egui::Color32::from_black_alpha(90),
        }
        .as_shape(loupe, radius),
    );
    painter.rect_filled(loupe, radius, egui::Color32::from_gray(18));
    if let Some(s) = sample {
        for (i, px) in s.pixels.iter().enumerate() {
            let Some([r, g, b]) = *px else { continue };
            let (x, y) = (
                (i as u32 % LOUPE_SIDE) as f32,
                (i as u32 / LOUPE_SIDE) as f32,
            );
            let cell = egui::Rect::from_min_size(
                loupe.min + egui::vec2(x * CELL, y * CELL),
                egui::vec2(CELL, CELL),
            );
            // The four corner cells take the rim's rounding, or their
            // square corners would show past it.
            let last = (LOUPE_SIDE - 1) as f32;
            let round = |on: bool| if on { radius.nw } else { 0 };
            let corners = egui::CornerRadius {
                nw: round(x == 0.0 && y == 0.0),
                ne: round(x == last && y == 0.0),
                sw: round(x == 0.0 && y == last),
                se: round(x == last && y == last),
            };
            painter.rect_filled(cell, corners, egui::Color32::from_rgb(r, g, b));
        }
        let mid = (LOUPE_SIDE / 2) as f32 * CELL;
        let hot =
            egui::Rect::from_min_size(loupe.min + egui::vec2(mid, mid), egui::vec2(CELL, CELL));
        painter.rect_stroke(
            hot.expand(1.0),
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_black_alpha(178)),
            egui::StrokeKind::Outside,
        );
        painter.rect_stroke(
            hot,
            0.0,
            egui::Stroke::new(1.5, egui::Color32::WHITE),
            egui::StrokeKind::Outside,
        );
    }
    painter.rect_stroke(
        loupe,
        radius,
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(190)),
        egui::StrokeKind::Outside,
    );
    painter.rect_stroke(
        loupe.expand(1.0),
        egui::CornerRadius::same(9),
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(150)),
        egui::StrokeKind::Outside,
    );
    let below = loupe.bottom() + CHIP_GAP + CHIP_H <= screen.bottom();
    let top = if below {
        loupe.bottom() + CHIP_GAP
    } else {
        loupe.top() - CHIP_GAP - CHIP_H
    };
    draw_chip(&painter, at.x, top, sample.and_then(Sample::center));
}

/// The chip under the loupe — swatch and hex — centred on `x` with its top at
/// `top`; with no colour, the "Off canvas" hint under the dropper cursor (L5).
fn draw_chip(painter: &egui::Painter, x: f32, top: f32, center: Option<[u8; 3]>) {
    let text = match center {
        Some([r, g, b]) => crate::ui::hex_of(Color::from_rgba8(r, g, b, 255)),
        None => "Off canvas".to_owned(),
    };
    let font = egui::FontId::proportional(10.5);
    let galley = painter.layout_no_wrap(
        text,
        font,
        if center.is_some() {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_white_alpha(170)
        },
    );
    let swatch = if center.is_some() { 10.0 + 5.0 } else { 0.0 };
    let w = galley.size().x + swatch + 12.0;
    let chip = egui::Rect::from_min_size(egui::pos2(x - w / 2.0, top), egui::vec2(w, CHIP_H));
    painter.rect(
        chip,
        egui::CornerRadius::same(5),
        egui::Color32::from_rgba_unmultiplied(18, 18, 20, 235),
        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(31)),
        egui::StrokeKind::Inside,
    );
    let mut x = chip.left() + 6.0;
    if let Some([r, g, b]) = center {
        let sw =
            egui::Rect::from_min_size(egui::pos2(x, chip.center().y - 5.0), egui::vec2(10.0, 10.0));
        painter.rect(
            sw,
            egui::CornerRadius::same(3),
            egui::Color32::from_rgb(r, g, b),
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(64)),
            egui::StrokeKind::Inside,
        );
        x += swatch;
    }
    painter.galley(
        egui::pos2(x, chip.center().y - galley.size().y / 2.0),
        galley,
        egui::Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    //! The eyedropper end to end on a headless app: real clicks through
    //! `canvas_ui`, sampled through the CPU path (`canvas::cpu_region`).
    //!
    //! Plain backticks: a `#[cfg(test)]` module is invisible to `cargo doc`.
    use super::*;
    use crate::panels::{PaintSlot, Picker};
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, Fill, NodeId, NodeKind, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };
    const BLUE: [u8; 3] = [0x6D, 0x8C, 0xD9];
    const ON_BLUE: egui::Pos2 = egui::pos2(250.0, 50.0);

    /// Two 100×100 rects at world (0,0) and (200,0) — on screen at the same
    /// points under the default camera. `a` is red at 40%, `b` opaque blue, and
    /// `a` is selected.
    fn app(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |id, index, x: f64| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
            },
            transform: Some(Affine::translate((x, 0.0))),
            name: None,
        };
        let paint = |id, c: Color| Operation::SetFills {
            id,
            fills: ondin_core::keyed_by_position([Fill {
                brush: Brush::Solid(c).into(),
                visible: true,
            }]),
        };
        let [r, g, bl] = BLUE;
        doc.apply(&Transaction(vec![
            rect(a, 0, 0.0),
            rect(b, 1, 200.0),
            paint(a, Color::from_rgba8(255, 0, 0, 102)),
            paint(b, Color::from_rgba8(r, g, bl, 255)),
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(a);
        (app, a, b)
    }

    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        held(ctx, app, events, egui::Modifiers::NONE);
    }

    /// One frame with `modifiers` held — what `i.modifiers` reads, which the
    /// button event's own field does not set.
    fn held(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                events,
                modifiers,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        );
    }

    /// Move to `at`, press and release the primary button there with
    /// `modifiers` held, and settle.
    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2, modifiers: egui::Modifiers) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        held(ctx, app, vec![button(true)], modifiers);
        held(ctx, app, vec![button(false)], modifiers);
        frame(ctx, app, Vec::new());
    }

    fn top_fill(app: &OndinApp, id: NodeId) -> Option<[u8; 4]> {
        let fills = &app.session.doc.get(id)?.paint().fills;
        match fills.last()?.brush.literal()? {
            Brush::Solid(c) => Some(c.to_rgba8().to_u8_array()),
            _ => None,
        }
    }

    /// **A rail pick recolours the selection's top fill and keeps its opacity**,
    /// as one undo step, and hands back to the tool it came from.
    /// flip: `land_pick` writing `Brush::Solid(color)` → fails at the alpha.
    #[test]
    fn a_rail_pick_paints_the_top_fill_and_keeps_its_opacity() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        app.choose_tool(Tool::Scale);
        app.choose_tool(Tool::Eyedropper);
        assert_eq!(app.dropper_armed(), Some(DropTarget::Selection));
        click(&ctx, &mut app, ON_BLUE, egui::Modifiers::NONE);
        let [r, g, b] = BLUE;
        assert_eq!(
            top_fill(&app, a),
            Some([r, g, b, 102]),
            "blue, at a's own 40%"
        );
        assert_eq!(
            app.tool,
            Tool::Scale,
            "one shot: back to the tool it came from"
        );
        assert_eq!(
            app.session.selection.ids(),
            &[a],
            "the click selected nothing"
        );
        assert!(app.session.undo(), "one step to undo");
        assert_eq!(top_fill(&app, a), Some([255, 0, 0, 102]));
        assert!(!app.session.undo(), "and only one");
    }

    /// **Shift keeps the rail tool armed** for another pick.
    /// flip: `land_pick` ignoring `stay` → fails at the tool.
    #[test]
    fn shift_keeps_it_armed() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        app.choose_tool(Tool::Eyedropper);
        click(&ctx, &mut app, ON_BLUE, egui::Modifiers::SHIFT);
        assert!(
            top_fill(&app, a).is_some_and(|c| c[..3] == BLUE),
            "the pick landed"
        );
        assert_eq!(app.tool, Tool::Eyedropper);
    }

    /// **Alt writes the top stroke**, adding one to a layer that has none, and
    /// leaves the fill alone.
    /// flip: `dropper_input` ignoring Alt → fails at the stroke.
    #[test]
    fn alt_paints_the_top_stroke() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        app.choose_tool(Tool::Eyedropper);
        click(&ctx, &mut app, ON_BLUE, egui::Modifiers::ALT);
        let node = app.session.doc.get(a).expect("a");
        let stroke = node.paint().strokes.last().expect("a stroke was added");
        let Some(Brush::Solid(c)) = stroke.brush.literal() else {
            panic!("a solid stroke")
        };
        assert_eq!(c.to_rgba8().to_u8_array()[..3], BLUE);
        assert_eq!(
            top_fill(&app, a),
            Some([255, 0, 0, 102]),
            "the fill is untouched"
        );
    }

    /// **With nothing selected the pick is copied, not written.**
    /// flip: `land_pick`'s empty-selection arm removed → fails at the status.
    #[test]
    fn nothing_selected_copies_the_hex_and_writes_nothing() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app(&ctx);
        app.session.selection.clear();
        app.choose_tool(Tool::Eyedropper);
        let revision = app.session.revision();
        click(&ctx, &mut app, ON_BLUE, egui::Modifiers::NONE);
        assert_eq!(
            app.session.status().text,
            "Copied 6D8CD9 · nothing selected"
        );
        assert_eq!(app.session.revision(), revision, "nothing written");
    }

    /// **`I` toggles the rail tool back to the tool it was armed from, and
    /// `Escape` does the same.**
    /// flip: `choose_tool`'s toggle arm removed → fails at the second `I`.
    #[test]
    fn i_and_escape_hand_back_to_the_previous_tool() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app(&ctx);
        app.choose_tool(Tool::Hand);
        app.choose_tool(Tool::Eyedropper);
        app.choose_tool(Tool::Eyedropper);
        assert_eq!(app.tool, Tool::Hand, "a second I");
        app.choose_tool(Tool::Eyedropper);
        app.escape(&ctx);
        assert_eq!(app.tool, Tool::Hand, "Escape");
        assert_eq!(
            app.session.selection.ids().len(),
            1,
            "and only one rung was spent"
        );
    }

    /// **A picker's pick writes its own slot**, keeps that paint's alpha, and
    /// leaves the picker open and disarmed.
    /// flip: `land_pick`'s `Picker` arm not disarming → fails at `picking`.
    #[test]
    fn a_picker_pick_writes_its_slot_and_stays_open() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        let mut picker = Picker::new(a, PaintSlot::Fill(0), egui::pos2(500.0, 300.0));
        picker.picking = true;
        app.picker = Some(picker);
        assert_eq!(app.dropper_armed(), Some(DropTarget::Picker));
        click(&ctx, &mut app, ON_BLUE, egui::Modifiers::NONE);
        let [r, g, b] = BLUE;
        assert_eq!(top_fill(&app, a), Some([r, g, b, 102]));
        let p = app.picker.as_ref().expect("still open");
        assert!(!p.picking, "disarmed by the pick");
        assert_eq!(app.tool, Tool::Select, "the tool never moved");
    }

    /// **Hovering previews into the picker only**: the picker reads the colour,
    /// the document does not.
    /// flip: `dropper_frame` not setting `preview` → fails at the preview.
    #[test]
    fn hovering_previews_in_the_picker_and_writes_nothing() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        let mut picker = Picker::new(a, PaintSlot::Fill(0), egui::pos2(500.0, 300.0));
        picker.picking = true;
        app.picker = Some(picker);
        let revision = app.session.revision();
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(ON_BLUE)]);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(app.picker.as_ref().and_then(|p| p.preview), Some(BLUE));
        assert_eq!(app.session.revision(), revision, "nothing written");
    }

    /// **`Escape` disarms the pick button before it closes the picker**: one
    /// rung a press.
    /// flip: the `disarm_dropper` rung removed from `escape` → fails at the
    /// picker being closed by the first press.
    #[test]
    fn escape_disarms_the_pick_button_before_closing_the_picker() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        let mut picker = Picker::new(a, PaintSlot::Fill(0), egui::pos2(500.0, 300.0));
        picker.picking = true;
        app.picker = Some(picker);
        app.escape(&ctx);
        let p = app
            .picker
            .as_ref()
            .expect("the first press leaves the picker open");
        assert!(!p.picking);
        app.escape(&ctx);
        assert!(app.picker.is_none(), "the second closes it");
    }

    /// **A pick reads afresh, never the hover's sample**: pixels that moved
    /// without the revision moving (typing, a preview) must not land stale.
    /// flip: `dropper_input` minting no `pick` number → fails at the colour.
    #[test]
    fn a_pick_reads_afresh_when_pixels_moved_under_a_still_pointer() {
        let ctx = egui::Context::default();
        let (mut app, a, b) = app(&ctx);
        app.choose_tool(Tool::Eyedropper);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(ON_BLUE)]);
        frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.canvas.sample().and_then(|s| s.center()),
            Some(BLUE),
            "fixture"
        );
        // Repaint b green behind the session's back: the pixels change and the
        // revision does not, which is what an uncommitted change looks like here.
        let revision = app.session.revision();
        app.session
            .doc
            .apply(&Transaction(vec![Operation::SetFills {
                id: b,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(0, 255, 0, 255)).into(),
                    visible: true,
                }]),
            }]))
            .expect("repaint b");
        assert_eq!(
            app.session.revision(),
            revision,
            "fixture: the revision stood still"
        );
        click(&ctx, &mut app, ON_BLUE, egui::Modifiers::NONE);
        assert_eq!(
            top_fill(&app, a),
            Some([0, 255, 0, 102]),
            "green, not the stale blue"
        );
    }

    /// **A picker's pick button stops the canvas answering as Select**, as the
    /// rail tool does by not selecting: no hover outline, no spacing band.
    /// flip: `selecting` reading only `Tool::selects` → fails at the picker.
    #[test]
    fn a_picking_picker_stops_the_canvas_selecting() {
        let ctx = egui::Context::default();
        let (mut app, a, _) = app(&ctx);
        assert!(app.selecting(), "fixture: Select, nothing armed");
        let mut picker = Picker::new(a, PaintSlot::Fill(0), egui::pos2(500.0, 300.0));
        picker.picking = true;
        app.picker = Some(picker);
        assert_eq!(app.tool, Tool::Select, "fixture: the tool never moved");
        assert!(!app.selecting());
    }

    fn gate(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> Vec<egui::Event> {
        let mut raw = egui::RawInput {
            events,
            ..Default::default()
        };
        app.gate_dropper_input(ctx, &mut raw);
        raw.events
    }

    fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    const SCREEN: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(1320.0, 820.0),
    };
    /// Over the canvas, the layers panel, the inspector and the picker under
    /// `SCREEN`.
    const ON_CANVAS: egui::Pos2 = egui::pos2(600.0, 300.0);
    const ON_LAYERS: egui::Pos2 = egui::pos2(150.0, 300.0);
    const ON_INSPECTOR: egui::Pos2 = egui::pos2(1205.0, 165.0);
    const ON_PICKER: egui::Pos2 = egui::pos2(1000.0, 400.0);

    /// The fixture with a picker picking, laid out by whole app frames so
    /// `layer_id_at` sees the real layers. A context that never drew a frame has
    /// none, and over none the gate's question has a different answer.
    fn laid_out(ctx: &egui::Context) -> OndinApp {
        crate::theme::install(ctx);
        let (mut app, a, _) = app(ctx);
        let mut picker = Picker::new(a, PaintSlot::Fill(0), egui::pos2(900.0, 300.0));
        picker.picking = true;
        app.picker = Some(picker);
        let mut f = eframe::Frame::_new_kittest();
        for _ in 0..3 {
            let raw = egui::RawInput {
                screen_rect: Some(SCREEN),
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| eframe::App::ui(&mut app, ui, &mut f));
        }
        let layer = |p| ctx.layer_id_at(p).map(|l| l.id);
        assert_eq!(
            layer(ON_CANVAS),
            Some(egui::Id::new("background")),
            "fixture"
        );
        assert_eq!(
            layer(ON_LAYERS),
            Some(egui::Id::new("background")),
            "fixture"
        );
        assert!(!app.canvas_rect.contains(ON_LAYERS), "fixture");
        assert_eq!(
            layer(ON_INSPECTOR),
            Some(egui::Id::new("inspector")),
            "fixture"
        );
        assert_eq!(
            layer(ON_PICKER),
            Some(crate::panels::picker_layer_id()),
            "fixture"
        );
        app
    }

    /// **Off the canvas an armed dropper hides the pointer from egui and spends
    /// its click and wheel** — over a panel and over the inspector — keeping the
    /// position for its own cursor; disarming hands the pointer back.
    /// flip: `gate_dropper_input`'s `live` answering true → fails at the gone.
    #[test]
    fn off_the_canvas_the_panels_get_no_pointer() {
        let ctx = egui::Context::default();
        let mut app = laid_out(&ctx);
        let wheel = egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        };
        let out = gate(
            &ctx,
            &mut app,
            vec![
                egui::Event::PointerMoved(ON_LAYERS),
                button(ON_LAYERS, true),
                button(ON_LAYERS, false),
                wheel,
                egui::Event::PointerMoved(ON_INSPECTOR),
                button(ON_INSPECTOR, true),
                button(ON_INSPECTOR, false),
            ],
        );
        assert_eq!(out, vec![egui::Event::PointerGone; 2]);
        assert_eq!(app.dropper_pointer, Some(ON_INSPECTOR));
        assert!(app.disarm_dropper(), "fixture: disarmed");
        let out = gate(&ctx, &mut app, Vec::new());
        assert_eq!(out, vec![egui::Event::PointerMoved(ON_INSPECTOR)]);
        assert_eq!(app.dropper_pointer, None);
    }

    /// **Over the canvas and the picking picker everything passes, and a press
    /// made on the canvas finishes off it** — a pan dragged past the canvas
    /// still gets its release.
    /// flip: `live` taking the background layer for chrome → fails at the first
    /// move; the `PointerMoved` arm ignoring `down` → fails at the drag.
    #[test]
    fn the_canvas_and_the_picker_get_the_pointer() {
        let ctx = egui::Context::default();
        let mut app = laid_out(&ctx);
        let events = vec![
            egui::Event::PointerMoved(ON_CANVAS),
            button(ON_CANVAS, true),
            egui::Event::PointerMoved(ON_LAYERS),
            button(ON_LAYERS, false),
            egui::Event::PointerMoved(ON_PICKER),
            button(ON_PICKER, true),
            button(ON_PICKER, false),
        ];
        assert_eq!(gate(&ctx, &mut app, events.clone()), events);
        assert_eq!(app.dropper_pointer, None);
    }

    /// **The hover sample is the pixel under the pointer, keyed to it**, and a
    /// request for that same key asks for no further frame.
    ///
    /// ⚠️ **Not the guard against a still pointer repainting forever**: that is
    /// `request_sample`'s GPU arm, and headless the CPU arm answers `false`
    /// whatever the key — flip: skipping the `fresh` check does not bite here.
    #[test]
    fn the_hover_sample_is_keyed_to_the_pointer() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app(&ctx);
        app.choose_tool(Tool::Eyedropper);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(ON_BLUE)]);
        frame(&ctx, &mut app, Vec::new());
        let key = app.canvas.sample().map(|s| s.key).expect("a sample");
        let (doc, res, _) = app.session.render_inputs();
        let camera = app.session.camera;
        assert!(
            !app.canvas
                .request_sample(key, doc, res, &camera, Color::WHITE)
        );
        assert_eq!(app.canvas.sample().map(|s| s.center()), Some(Some(BLUE)));
    }
}
