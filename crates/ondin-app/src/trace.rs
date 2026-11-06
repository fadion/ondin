//! *Trace image…* (`docs/design/images.md`, *Trace image*): a card over the canvas
//! that turns a picture layer into filled paths (`ondin_core::trace`), comparing
//! the picture with its trace as the settings move, and committing a *Trace*
//! group above the picture as one *Trace image* step.
//!
//! The picture is read as shown, crop and adjustments included: the card first
//! renders the layer on a worker, then traces that raster on a worker whenever
//! the settings change, one trace at a time, as the Offset card works.

use crate::app::OndinApp;
use crate::job::Job;
use crate::theme::{self, color, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use ondin_core::NodeId;
use ondin_core::kurbo::{BezPath, Rect};
use ondin_core::trace::{self, Labels, Mode, Settings, Traced};
use std::sync::Arc;

/// The card's width: batch rename's.
const W: f32 = 640.0;
/// The settings column.
const SIDE_W: f32 = 236.0;
/// The preview's height.
const PREVIEW_H: f32 = 262.0;
const PAD: f32 = 16.0;
/// The raster's longer side, at most: enough for a logo's curves, few enough
/// pixels that a trace lands while the setting is still being scrubbed.
const MAX_SIDE: f64 = 1200.0;
/// The most a small picture is enlarged to trace it.
const MAX_SCALE: f64 = 4.0;

/// The picture, rendered as shown, and where it lies.
struct Shot {
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    /// The world box the raster covers, at `scale` pixels a unit.
    frame: Rect,
    scale: f64,
}

/// What one trace made: the labels for the preview's fill, and the paths.
struct Made {
    labels: Labels,
    traced: Traced,
}

/// The card while it is open.
pub(crate) struct TraceCard {
    picture: NodeId,
    revision: u64,
    pub(crate) settings: Settings,
    /// Each mode's own number, kept as the mode switches.
    threshold: f64,
    colours: f64,
    /// Keep the picture under the trace.
    pub(crate) keep: bool,
    /// Where the preview splits picture from trace, `0..=1` of its width.
    divider: f32,
    shooting: Option<Job<Option<Shot>>>,
    shot: Option<Arc<Shot>>,
    /// The render came back with nothing: the card says so rather than waiting.
    unreadable: bool,
    picture_tex: Option<egui::TextureHandle>,
    working: Option<(Settings, Job<Made>)>,
    landed: Option<(Settings, Arc<Made>)>,
    trace_tex: Option<egui::TextureHandle>,
}

impl TraceCard {
    /// The settings the card's controls say, in raster pixels.
    fn asked(&self) -> Settings {
        let mut s = self.settings;
        s.mode = match s.mode {
            Mode::BlackAndWhite { .. } => Mode::BlackAndWhite {
                threshold: self.threshold / 100.0,
            },
            Mode::Colours { .. } => Mode::Colours {
                count: self.colours.round() as usize,
            },
        };
        if let Some(shot) = &self.shot {
            s.speck *= shot.scale;
        }
        s
    }

    /// Whether the trace on show is the one the settings ask for.
    fn done(&self) -> bool {
        self.landed
            .as_ref()
            .is_some_and(|(s, _)| *s == self.asked())
    }

    /// Take in what landed and set the next work going: the picture's render
    /// first, then a trace of it whenever the settings moved.
    fn pump(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.shooting {
            match job.poll() {
                Ok(Some(shot)) => {
                    self.shooting = None;
                    self.unreadable = shot.is_none();
                    self.shot = shot.map(Arc::new);
                    if let Some(shot) = &self.shot {
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [shot.width, shot.height],
                            &shot.rgba,
                        );
                        self.picture_tex = Some(ctx.load_texture(
                            "trace-picture",
                            image,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                Ok(None) => {}
                Err(()) => {
                    self.shooting = None;
                    self.unreadable = true;
                }
            }
        }
        if let Some((s, job)) = &self.working {
            match job.poll() {
                Ok(Some(made)) => {
                    let image = label_image(&made.labels);
                    self.trace_tex = Some(ctx.load_texture(
                        "trace-labels",
                        image,
                        egui::TextureOptions::NEAREST,
                    ));
                    self.landed = Some((*s, Arc::new(made)));
                    self.working = None;
                }
                Ok(None) => {}
                Err(()) => self.working = None,
            }
        }
        if self.working.is_none()
            && !self.done()
            && let Some(shot) = &self.shot
        {
            let (shot, s) = (Arc::clone(shot), self.asked());
            let job = Job::spawn("ondin-trace", ctx, move || made(&shot, &s));
            self.working = Some((s, job));
        }
    }

    /// Make the asked-for trace land, waiting on the worker if it has it in hand
    /// and tracing here otherwise: the commit places what the settings say.
    fn settle(&mut self) {
        if let Some(job) = self.shooting.take() {
            self.shot = job.wait().flatten().map(Arc::new);
        }
        if self.done() {
            return;
        }
        let Some(shot) = self.shot.clone() else {
            return;
        };
        let s = self.asked();
        let m = match self.working.take() {
            Some((w, job)) if w == s => job.wait(),
            _ => None,
        };
        let m = m.unwrap_or_else(|| made(&shot, &s));
        self.landed = Some((s, Arc::new(m)));
    }
}

/// One trace of the shot.
fn made(shot: &Shot, s: &Settings) -> Made {
    let bitmap = trace::Bitmap {
        rgba: &shot.rgba,
        width: shot.width,
        height: shot.height,
    };
    let labels = trace::label(bitmap, s);
    let traced = trace::trace(&labels, s);
    Made { labels, traced }
}

/// The labels painted in their colours, the empty ones clear.
fn label_image(l: &Labels) -> egui::ColorImage {
    let mut rgba = Vec::with_capacity(l.index.len() * 4);
    for &k in &l.index {
        match l.palette.get(usize::from(k)) {
            Some(c) => rgba.extend_from_slice(&c.to_rgba8().to_u8_array()),
            None => rgba.extend_from_slice(&[0, 0, 0, 0]),
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([l.width, l.height], &rgba)
}

enum Act {
    None,
    Cancel,
    Trace,
}

impl OndinApp {
    /// The picture layer *Trace image…* would trace: the one selected layer, when
    /// it shows a picture.
    pub(crate) fn trace_subject(&self) -> Option<NodeId> {
        let id = self.session.selection.single()?;
        let node = self.session.doc.get(id)?;
        node.paint().image_fill().is_some().then_some(id)
    }

    /// Open the card on the selected picture, rendering it on a worker.
    pub(crate) fn open_trace(&mut self) {
        let Some(id) = self.trace_subject() else {
            return self.session.info("Select a picture to trace");
        };
        if let Some(why) = trace::refusal(&self.session.doc, id) {
            return self.session.info(why);
        }
        let (doc, res) = (self.session.doc.clone(), self.session.resolved.clone());
        let shooting = Job::spawn("ondin-trace-shot", &self.egui_ctx, move || {
            shoot(&doc, &res, id)
        });
        self.trace = Some(TraceCard {
            picture: id,
            revision: self.session.revision(),
            settings: Settings::default(),
            threshold: 50.0,
            colours: 4.0,
            keep: true,
            divider: 0.5,
            shooting: Some(shooting),
            shot: None,
            unreadable: false,
            picture_tex: None,
            working: None,
            landed: None,
            trace_tex: None,
        });
    }

    /// Place the trace the settings ask for, one *Trace image* step, and select it.
    pub(crate) fn commit_trace(&mut self, mut card: TraceCard) {
        card.settle();
        let (Some(shot), Some((_, m))) = (&card.shot, &card.landed) else {
            return self.session.info("Nothing to trace");
        };
        let placed = trace::place(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            card.picture,
            &m.traced,
            shot.frame,
            shot.scale,
            card.keep,
        );
        let (tx, group) = match placed {
            Ok(placed) => placed,
            Err(why) => return self.session.info(why),
        };
        self.session.name_step("Trace image", icon::TRACE);
        if self.commit_edit(tx) {
            self.session.selection.set_one(group);
            self.session.info(format!(
                "Traced into {}",
                ui::count_as(m.traced.paths(), "path", "paths")
            ));
        }
    }

    /// The card over the canvas while it is open: the preview, the settings and
    /// the footer. Escape, the ×, *Cancel* or a change under it closes it.
    pub(crate) fn trace_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect) {
        let Some(mut card) = self.trace.take() else {
            return;
        };
        if card.revision != self.session.revision() || self.session.doc.get(card.picture).is_none()
        {
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            return;
        }
        let enter = ui::card_enter(ctx);
        card.pump(ctx);
        let area = ui::canvas_popup_area(ctx, egui::Id::new("trace-image"), canvas);
        let mut act = Act::None;
        let mut modal = crate::settings::card_modal(ctx, "trace-image")
            .area(area)
            .backdrop_color(egui::Color32::TRANSPARENT);
        modal.frame = modal.frame.map(|f| f.inner_margin(egui::Margin::ZERO));
        let modal = modal.show(ctx, |ui| {
            ui.set_width(W);
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 40.0), egui::Sense::hover());
            let mut child = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if crate::settings::modal_title(&mut child, "Trace image") {
                act = Act::Cancel;
            }
            ui::card_rule(ui);
            egui::Frame::NONE
                .inner_margin(egui::Margin::same(PAD as i8))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = PAD;
                        let preview_w = W - 3.0 * PAD - SIDE_W;
                        preview(ui, &mut card, preview_w);
                        ui.vertical(|ui| {
                            ui.set_width(SIDE_W);
                            settings(ui, &mut card);
                        });
                    });
                });
            ui::card_rule(ui);
            if let Some(a) = foot(ui, &card) {
                act = a;
            }
            ui::hold_tab(ui);
        });
        if modal.backdrop_response.clicked() {
            return;
        }
        if enter && matches!(act, Act::None) {
            act = Act::Trace;
        }
        match act {
            Act::Cancel => {}
            Act::Trace => {
                self.commit_trace(card);
                ctx.request_repaint();
            }
            Act::None => self.trace = Some(card),
        }
    }
}

/// The picture layer rendered as shown, at a scale that keeps its longer side
/// near [`MAX_SIDE`] pixels.
fn shoot(doc: &ondin_core::Document, res: &ondin_core::Resolved, id: NodeId) -> Option<Shot> {
    let frame = ondin_export::extent(res, &[id])?;
    let long = frame.width().max(frame.height());
    if long <= 0.0 {
        return None;
    }
    let scale = (MAX_SIDE / long).min(MAX_SCALE);
    let opts = ondin_export::png::RasterOpts {
        scale,
        ..Default::default()
    };
    let r = ondin_export::png::raster_of(doc, res, &[id], &opts)?;
    Some(Shot {
        rgba: r.rgba,
        width: r.width as usize,
        height: r.height as usize,
        frame,
        scale,
    })
}

/// The preview: the picture left of the divider and the trace right of it, its
/// outlines drawn over its fill. Dragging anywhere in it moves the divider.
fn preview(ui: &mut egui::Ui, card: &mut TraceCard, w: f32) {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, PREVIEW_H), egui::Sense::drag());
    let painter = ui.painter_at(rect);
    // A field's ground and corner: the picture sits in a well.
    painter.rect_filled(rect, egui::CornerRadius::same(ui::BUTTON_R), color::field());
    let Some(shot) = &card.shot else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            unread(card),
            egui::FontId::proportional(ui::SMALL_PT),
            theme::text::faint(),
        );
        return;
    };
    let fit = (rect.width() / shot.width as f32).min(rect.height() / shot.height as f32) * 0.92;
    let shown = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(shot.width as f32, shot.height as f32) * fit,
    );
    if let Some(p) = resp.interact_pointer_pos() {
        card.divider = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let split = rect.left() + rect.width() * card.divider;
    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
    let left = painter.with_clip_rect(egui::Rect::from_min_max(
        rect.min,
        egui::pos2(split, rect.bottom()),
    ));
    let right = painter.with_clip_rect(egui::Rect::from_min_max(
        egui::pos2(split, rect.top()),
        rect.max,
    ));
    if let Some(tex) = &card.picture_tex {
        left.image(tex.id(), shown, uv, egui::Color32::WHITE);
    }
    if let Some(tex) = &card.trace_tex {
        right.image(tex.id(), shown, uv, egui::Color32::WHITE);
    }
    if let Some((_, m)) = &card.landed {
        let to_screen = |p: ondin_core::kurbo::Point| {
            egui::pos2(
                shown.left() + p.x as f32 * fit,
                shown.top() + p.y as f32 * fit,
            )
        };
        let stroke = egui::Stroke::new(1.0, color::SELECT);
        for path in m.traced.layers.iter().flat_map(|l| &l.paths) {
            outline(&right, path, &to_screen, stroke);
        }
    }
    painter.line_segment(
        [
            egui::pos2(split, rect.top()),
            egui::pos2(split, rect.bottom()),
        ],
        egui::Stroke::new(1.5, color::bg()),
    );
    let grip =
        egui::Rect::from_center_size(egui::pos2(split, rect.center().y), egui::vec2(8.0, 20.0));
    painter.rect_filled(grip, 3.0, color::bg());
}

/// `path`'s outline, flattened, through `to_screen`.
fn outline(
    painter: &egui::Painter,
    path: &BezPath,
    to_screen: &impl Fn(ondin_core::kurbo::Point) -> egui::Pos2,
    stroke: egui::Stroke,
) {
    use ondin_core::kurbo::PathEl;
    let mut run: Vec<egui::Pos2> = Vec::new();
    let flush = |run: &mut Vec<egui::Pos2>| {
        if run.len() > 1 {
            painter.add(egui::Shape::closed_line(std::mem::take(run), stroke));
        }
        run.clear();
    };
    ondin_core::kurbo::flatten(path, 0.5, |el| match el {
        PathEl::MoveTo(p) => {
            flush(&mut run);
            run.push(to_screen(p));
        }
        PathEl::LineTo(p) => run.push(to_screen(p)),
        PathEl::ClosePath => flush(&mut run),
        _ => {}
    });
    flush(&mut run);
}

/// The settings column: the mode, then its threshold or its colours on one row
/// that swaps with the mode, the detail, the corners, the specks and the switch.
fn settings(ui: &mut egui::Ui, card: &mut TraceCard) {
    ui.spacing_mut().item_spacing.y = 11.0;
    let colours = matches!(card.settings.mode, Mode::Colours { .. });
    if let Some(i) = ui::segmented(
        ui,
        SIDE_W,
        ui::SEGMENT_CELL_H,
        2,
        usize::from(colours),
        |p, i, r, on| {
            ui::segment_label(p, r, ["Black and white", "Colours"][i], on);
        },
    ) {
        card.settings.mode = match i {
            0 => Mode::BlackAndWhite { threshold: 0.5 },
            _ => Mode::Colours { count: 4 },
        };
    }
    let size = egui::vec2(SIDE_W, ui::CONTROL_H);
    if colours {
        ui::labelled(ui, "Colours", |ui| {
            ui::value_field(
                ui,
                size,
                Prefix::Text("#"),
                &mut card.colours,
                Scrub::whole(0.1).range(2.0..=trace::MAX_COLOURS as f64),
                |d| d.custom_formatter(ui::number(0)),
            )
            .on_hover_text("How many colours the picture is cut down to")
        });
    } else {
        ui::labelled(ui, "Threshold", |ui| {
            ui::value_field(
                ui,
                size,
                Prefix::Text("%"),
                &mut card.threshold,
                Scrub::whole(0.5).range(1.0..=99.0),
                percent,
            )
            .on_hover_text("Pixels darker than this become black")
        });
    }
    let mut detail = card.settings.detail * 100.0;
    ui::labelled(ui, "Detail", |ui| {
        ui::value_field(
            ui,
            size,
            Prefix::Text("%"),
            &mut detail,
            Scrub::whole(0.5).range(0.0..=100.0),
            percent,
        )
        .on_hover_text("How closely the curves follow the picture")
    });
    card.settings.detail = detail / 100.0;
    let mut corners = card.settings.corners * 100.0;
    ui::labelled(ui, "Corners", |ui| {
        ui::value_field(
            ui,
            size,
            Prefix::Text("%"),
            &mut corners,
            Scrub::whole(0.5).range(0.0..=100.0),
            percent,
        )
        .on_hover_text("How readily a turn stays a sharp corner")
    });
    card.settings.corners = corners / 100.0;
    ui::labelled(ui, "Ignore specks under", |ui| {
        ui::value_field(
            ui,
            size,
            Prefix::Text("px"),
            &mut card.settings.speck,
            Scrub::whole(0.1).range(0.0..=100.0),
            |d| d.custom_formatter(ui::number(1)),
        )
        .on_hover_text("Spots and holes smaller than this are left out")
    });
    if ui::switch_row(ui, "Keep the picture", card.keep, crate::settings::SWITCH_H).clicked() {
        card.keep = !card.keep;
    }
}

/// What the card says while it has no picture to show: still reading it, or
/// unable to.
fn unread(card: &TraceCard) -> &'static str {
    match card.unreadable {
        true => "This picture can't be read to trace",
        false => "Still reading the picture",
    }
}

/// A percent field's number: whole, with a percent sign.
fn percent(d: egui::DragValue<'_>) -> egui::DragValue<'_> {
    d.custom_formatter(|v, _| format!("{v:.0}%"))
}

/// The footer: the count, or *Tracing…* until it catches up, then *Cancel* and
/// *Trace*.
fn foot(ui: &mut egui::Ui, card: &TraceCard) -> Option<Act> {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 48.0), egui::Sense::hover());
    let note = match &card.landed {
        Some((_, m)) if card.done() => format!(
            "{} · {}",
            ui::count_as(m.traced.paths(), "path", "paths"),
            ui::count_as(m.traced.points(), "point", "points")
        ),
        _ => "Tracing…".to_string(),
    };
    ui.painter().text(
        egui::pos2(rect.left() + PAD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        note,
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::muted(),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 10.0)))
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    let why_not = card.shot.is_none().then(|| unread(card));
    let state = match why_not {
        None => FieldButton::Primary,
        Some(_) => FieldButton::Disabled,
    };
    let buttons = [("Cancel", FieldButton::Off), ("Trace", state)];
    match crate::settings::footer_row_noted(&mut child, &buttons, why_not) {
        Some(0) => Some(Act::Cancel),
        Some(_) if why_not.is_none() => Some(Act::Trace),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Shape, Size};
    use ondin_core::{
        Document, Fill, IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, NodeKind,
        Operation, Transaction, image_brush,
    };

    /// A headless app holding an 80×60 picture at (100, 50), selected: a 40×30 PNG
    /// of a black disc of radius 10 on white, so the disc lands at (140, 80) with
    /// radius 20.
    fn app_with_a_disc(ctx: &egui::Context) -> (OndinApp, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let (w, h) = (40u32, 30u32);
        let rgba: Vec<u8> = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                let (dx, dy) = (f64::from(x) + 0.5 - 20.0, f64::from(y) + 0.5 - 15.0);
                if dx * dx + dy * dy < 100.0 {
                    [0, 0, 0, 255]
                } else {
                    [255, 255, 255, 255]
                }
            })
            .collect();
        let bytes = ondin_export::png::encode_rgba8(&rgba, w, h);
        let mut ids = IdSource::new(0x7A);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let rect = ids.mint();
        let image = ImageId("a-disc".into());
        doc.apply(&Transaction(vec![
            Operation::AddImage {
                id: image.clone(),
                entry: ImageEntry {
                    source: ImageSource::Embedded(bytes.into()),
                    format: ImageFormat::Png,
                    width: w,
                    height: h,
                },
            },
            Operation::CreateNode {
                id: rect,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(80.0, 60.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((100.0, 50.0))),
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: image_brush(image).into(),
                    visible: true,
                }]),
            },
        ]))
        .expect("build");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(rect);
        (app, rect)
    }

    /// The picture traces to one path where the disc is drawn, in a *Trace* group
    /// just above the picture, as one *Trace image* step that selects the group;
    /// the picture stays.
    #[test]
    fn a_disc_traces_to_one_path_above_the_picture() {
        let ctx = egui::Context::default();
        let (mut app, picture) = app_with_a_disc(&ctx);
        app.open_trace();
        let card = app.trace.take().expect("opened");
        let depth = app.session.history.undo_depth();
        app.commit_trace(card);
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Trace image");
        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        let page = app.session.doc.first_page();
        let kids = app.session.doc.get(page).unwrap().children().to_vec();
        assert_eq!(kids, vec![picture, group], "just above the picture");
        let g = app.session.doc.get(group).unwrap();
        assert_eq!(g.name(), trace::GROUP_NAME);
        let paths = g.children().to_vec();
        assert_eq!(paths.len(), 1, "one disc");
        let b = app.session.resolved.world_bounds(paths[0]).expect("drawn");
        for (got, want) in [(b.x0, 120.0), (b.x1, 160.0), (b.y0, 60.0), (b.y1, 100.0)] {
            assert!((got - want).abs() < 2.5, "{b:?}");
        }
        let node = app.session.doc.get(paths[0]).unwrap();
        assert_eq!(node.paint().fills.len(), 1);
        assert!(node.paint().strokes.is_empty());
        let area = app
            .session
            .resolved
            .world_transform(paths[0])
            .map(|w| {
                (w * app
                    .session
                    .resolved
                    .local_path(&app.session.doc, paths[0])
                    .unwrap())
                .area()
                .abs()
            })
            .unwrap();
        let want = std::f64::consts::PI * 400.0;
        assert!((area - want).abs() < 0.05 * want, "{area} against {want}");
    }

    /// Without *Keep the picture*, the trace takes the picture's place.
    #[test]
    fn without_keep_the_picture_goes() {
        let ctx = egui::Context::default();
        let (mut app, picture) = app_with_a_disc(&ctx);
        app.open_trace();
        let mut card = app.trace.take().expect("opened");
        card.keep = false;
        app.commit_trace(card);
        assert!(app.session.doc.get(picture).is_none());
        assert!(app.session.selection.single().is_some());
    }

    /// A layer with no picture opens no card and says why; the card closes when the
    /// document changes under it.
    #[test]
    fn only_a_picture_opens_the_card() {
        let ctx = egui::Context::default();
        let (mut app, picture) = app_with_a_disc(&ctx);
        app.session.selection.clear();
        app.open_trace();
        assert!(app.trace.is_none());
        app.session.selection.set_one(picture);
        app.open_trace();
        assert!(app.trace.is_some());
        let rename = Transaction(vec![Operation::SetName {
            id: picture,
            name: "Moved on".into(),
        }]);
        assert!(app.session.commit(rename));
        let _ = ctx.run_ui(Default::default(), |ui| {
            app.trace_ui(
                ui.ctx(),
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0)),
            )
        });
        assert!(app.trace.is_none(), "an edit under it closes it");
    }
}
