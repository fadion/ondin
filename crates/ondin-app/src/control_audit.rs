//! The form-control standard, measured on the rendered UI: every painted ground
//! that sits on a clickable or focusable widget must have a standard height,
//! corner and colour, however it was drawn. The backstop for what clippy cannot
//! see — a hand-painted ground is checked like a primitive's.

use crate::app::OndinApp;
use crate::theme::color;
use crate::ui;
use eframe::egui;
use egui::epaint::{CornerRadius, RectShape, Shape, Stroke};
use egui::{Color32, Rect};
use ondin_core::kurbo::{Affine, Size};
use ondin_core::{Document, NodeId, NodeKind, Operation, Transaction};

/// Tall enough that the inspector does not scroll its cards out of the pass.
const SCREEN: egui::Vec2 = egui::vec2(1320.0, 1600.0);

/// How far a painted rect may sit from the widget's rect and still be its ground.
const SLOP: f32 = 0.5;

/// One whole app frame, the clock a second on so every fade has finished (a
/// ground caught mid-fade carries a partial alpha no standard colour has).
fn pass(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> egui::FullOutput {
    let mut frame = eframe::Frame::_new_kittest();
    let time = ctx.input(|i| i.time) + 1.0;
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
        time: Some(time),
        events,
        ..Default::default()
    };
    ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
}

/// Several frames, so interaction state and late-resolving rects settle; returns
/// the last one's output.
fn settle(ctx: &egui::Context, app: &mut OndinApp) -> egui::FullOutput {
    for _ in 0..4 {
        pass(ctx, app, Vec::new());
    }
    pass(ctx, app, Vec::new())
}

/// A widget registered this pass that takes a click or the keyboard.
#[derive(Clone, Copy)]
struct Control {
    /// In screen coordinates.
    rect: Rect,
    enabled: bool,
    /// Senses a drag: a text entry or a scrubbed number, not a button.
    drags: bool,
}

fn controls(ctx: &egui::Context) -> Vec<Control> {
    // Read the rects first, then the transforms: Context accessors never nest.
    let local: Vec<(egui::LayerId, Control)> = ctx.viewport(|v| {
        v.this_pass
            .widgets
            .layers()
            .flat_map(|(_, ws)| ws.iter())
            .filter(|w| w.sense.senses_click() || w.sense.is_focusable())
            .map(|w| {
                let c = Control {
                    rect: w.rect,
                    enabled: w.enabled,
                    drags: w.sense.senses_drag(),
                };
                (w.layer_id, c)
            })
            .collect()
    });
    local
        .into_iter()
        .map(|(layer, c)| match ctx.layer_transform_to_global(layer) {
            Some(t) => Control {
                rect: t * c.rect,
                ..c
            },
            None => c,
        })
        .collect()
}

fn walk<'a>(shape: &'a Shape, clip: Rect, out: &mut Vec<(&'a Shape, Rect)>) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| walk(s, clip, out)),
        s => out.push((s, clip)),
    }
}

fn flat(out: &egui::FullOutput) -> Vec<(&Shape, Rect)> {
    let mut all = Vec::new();
    for c in &out.shapes {
        walk(&c.shape, c.clip_rect, &mut all);
    }
    all
}

/// Every rect shape on screen, with its place in the paint order.
fn rect_shapes(out: &egui::FullOutput) -> Vec<(usize, RectShape)> {
    flat(out)
        .into_iter()
        .enumerate()
        .filter_map(|(i, (s, clip))| match s {
            // A ground wholly clipped away is not on screen.
            Shape::Rect(r) if clip.intersects(r.rect) => Some((i, r.clone())),
            _ => None,
        })
        .collect()
}

/// A run of painted text: where, what, in which ink, and its place in the paint
/// order (text painted before a ground lies under it, behind a modal say).
struct Ink {
    rect: Rect,
    text: String,
    color: Color32,
    order: usize,
}

fn texts(out: &egui::FullOutput) -> Vec<Ink> {
    flat(out)
        .into_iter()
        .enumerate()
        .filter_map(|(order, (s, _))| match s {
            Shape::Text(t) => Some(Ink {
                order,
                rect: t.visual_bounding_rect(),
                text: t.galley.text().trim().to_owned(),
                color: t.override_text_color.unwrap_or_else(|| {
                    t.galley
                        .job
                        .sections
                        .first()
                        .map(|s| s.format.color)
                        .filter(|c| *c != Color32::PLACEHOLDER)
                        .unwrap_or(t.fallback_color)
                }),
            }),
            _ => None,
        })
        .filter(|t| !t.text.is_empty())
        .collect()
}

/// How `ground` holds the control at `widget`, if it does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Holds {
    /// The widget fills its height, less a value field's border (a
    /// `value_field` paints its box round a `DragValue` inset by
    /// `ui::FIELD_BORDER_H`): the ground is the control's.
    Whole,
    /// The widget is a cell inset by a segmented track's padding and hairline:
    /// the ground is the track, which takes no hover of its own.
    Cell,
}

/// What a segmented track gives up round its cells, top and bottom together.
const TRACK_INSET: f32 = ui::CONTROL_H - (ui::SEGMENT_CELL_H - 2.0);

/// A card holding a button holds neither.
fn holds(ground: Rect, widget: Rect) -> Option<Holds> {
    if !ground.expand(SLOP).contains_rect(widget) {
        return None;
    }
    let short = ground.height() - widget.height();
    if short <= ui::FIELD_BORDER_H + SLOP {
        Some(Holds::Whole)
    } else if (short - TRACK_INSET).abs() <= SLOP {
        Some(Holds::Cell)
    } else {
        None
    }
}

fn stroked(s: &RectShape) -> bool {
    s.stroke.width > 0.0 && s.stroke.color != Color32::TRANSPARENT
}

/// A painted ground on a control, with what it was painted with.
#[derive(Clone, Debug)]
struct Ground {
    rect: Rect,
    radius: CornerRadius,
    fill: Color32,
    stroke: Stroke,
    label: String,
    /// Whether a widget it hosts is enabled and not dimmed, so it owes a hover.
    live: bool,
    /// Whether it hosts a text entry or a scrubbed number, which keeps its
    /// resting ground under the pointer.
    field: bool,
}

impl std::fmt::Display for Ground {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let r = self.rect;
        write!(
            f,
            "{:?} at ({:.1},{:.1}) {:.1}x{:.1} h={:.1} r={:?} fill={} stroke={}@{:.1}",
            self.label,
            r.min.x,
            r.min.y,
            r.width(),
            r.height(),
            r.height(),
            self.radius,
            hex(self.fill),
            hex(self.stroke.color),
            self.stroke.width,
        )
    }
}

fn hex(c: Color32) -> String {
    let [r, g, b, a] = c.to_array();
    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
}

/// The text painted on the ground at `rect` (painted after it, at `order`), to
/// name the control in a report.
fn label_in(rect: Rect, order: usize, texts: &[Ink]) -> String {
    inks_in(rect, order, texts)
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn inks_in(rect: Rect, order: usize, texts: &[Ink]) -> impl Iterator<Item = &Ink> {
    texts
        .iter()
        .filter(move |t| t.order > order && rect.expand(1.0).contains(t.rect.center()))
}

/// Whether everything written on the ground is in the disabled ink: a control
/// shown disabled by paint alone (`ui::FieldButton::Disabled`), which owes no
/// hover.
fn dimmed(rect: Rect, order: usize, texts: &[Ink]) -> bool {
    let mut inks = inks_in(rect, order, texts).peekable();
    inks.peek().is_some() && inks.all(|t| t.color == crate::theme::text::disabled())
}

/// Whether a ground is a surface controls sit on rather than a control: taller
/// than any control, or painted a container's ground (a folded card is a
/// clickable head the height of the card).
fn surface(s: &RectShape) -> bool {
    s.rect.height() > 2.0 * ui::CONTROL_H || [color::card(), color::raised()].contains(&s.fill)
}

/// Every non-transparent ground painted on a control this pass. A fill and a
/// hairline painted as two shapes on one rect are one ground; the later paint
/// wins, as it does on screen (a focused field's accent hairline over its
/// resting one).
fn grounds(ctx: &egui::Context, out: &egui::FullOutput) -> Vec<Ground> {
    let controls = controls(ctx);
    let texts = texts(out);
    let mut found: Vec<Ground> = Vec::new();
    for (order, s) in rect_shapes(out)
        .into_iter()
        .filter(|(_, s)| s.fill != Color32::TRANSPARENT || stroked(s))
        .filter(|(_, s)| s.brush.is_none() && s.blur_width == 0.0 && !surface(s))
    {
        let held: Vec<(Control, Holds)> = controls
            .iter()
            .filter_map(|c| holds(s.rect, c.rect).map(|h| (*c, h)))
            .collect();
        if held.is_empty() {
            continue;
        }
        let whole: Vec<Control> = held
            .iter()
            .filter(|(_, h)| *h == Holds::Whole)
            .map(|(c, _)| *c)
            .collect();
        let at = found
            .iter()
            .position(|g| g.rect == s.rect && g.radius == s.corner_radius);
        let g = match at {
            Some(i) => &mut found[i],
            None => {
                found.push(Ground {
                    rect: s.rect,
                    radius: s.corner_radius,
                    fill: Color32::TRANSPARENT,
                    stroke: Stroke::NONE,
                    label: label_in(s.rect, order, &texts),
                    live: false,
                    field: false,
                });
                found.last_mut().expect("just pushed")
            }
        };
        g.live |= whole.iter().any(|c| c.enabled) && !dimmed(s.rect, order, &texts);
        g.field |= whole.iter().any(|c| c.drags);
        if s.fill != Color32::TRANSPARENT {
            g.fill = s.fill;
        }
        if stroked(&s) {
            g.stroke = s.stroke;
        }
    }
    found
}

/// A height the standard allows and the corners it allows at it.
struct Rung {
    h: f32,
    radii: &'static [u8],
    what: &'static str,
}

/// The standard's heights, each from the `ui` const that sets it.
fn heights() -> Vec<Rung> {
    // A segment cell is the track less its padding and hairline on each side.
    let cell = ui::SEGMENT_CELL_H - 2.0;
    let toolbar_cell = ui::TOOLBAR_CELL_H - 2.0;
    vec![
        Rung {
            h: ui::CONTROL_H,
            radii: &[ui::BUTTON_R],
            what: "control",
        },
        Rung {
            h: ui::TOOLBAR_H,
            radii: &[ui::BUTTON_R],
            what: "toolbar control",
        },
        Rung {
            h: ui::MENU_ROW_H,
            radii: &[4],
            what: "list row",
        },
        Rung {
            h: cell,
            radii: &[4],
            what: "segment cell",
        },
        Rung {
            h: toolbar_cell,
            radii: &[4],
            what: "toolbar segment cell",
        },
        Rung {
            h: ui::FIELD_TOGGLE.y,
            radii: &[4],
            what: "field toggle",
        },
    ]
}

/// A ground the standard deliberately leaves off its scale: what it is, why, and
/// how to recognise it. Nothing here is a drift being waved through.
struct Exempt {
    what: &'static str,
    why: &'static str,
    is: fn(&Ground) -> bool,
}

/// Small enough to be a chip rather than a control: under a badge's height.
fn chip(g: &Ground) -> bool {
    let r = g.rect;
    (r.width() - r.height()).abs() <= SLOP && r.height() < ui::BADGE_FIELD_H
}

/// An active `ui::icon_button`: a square lit `accent_800()` inside an `accent()`
/// ring, a pair no form-control state paints.
fn active_icon_button(g: &Ground) -> bool {
    let r = g.rect;
    (r.width() - r.height()).abs() <= SLOP
        && g.fill == color::accent_800()
        && g.stroke.color == color::accent()
}

const EXEMPT: &[Exempt] = &[
    Exempt {
        what: "swatch chip",
        why: "shows the paint itself (a checker under the colour, a ring round it), \
              not a control state; the standard names swatch chips as deliberately shorter",
        is: chip,
    },
    Exempt {
        what: "active icon button",
        why: "`ui::icon_button` (the rail, the top bar, card heads) is its own family \
              with its own lit state, outside the form-control standard",
        is: active_icon_button,
    },
];

fn fills() -> Vec<Color32> {
    vec![
        color::field(),
        color::hover(),
        color::accent_900(),
        color::accent_800(),
        color::accent(),
        color::accent_400(),
        color::danger_900(),
        color::danger_800(),
        color::text_a(33),
        Color32::TRANSPARENT,
    ]
}

fn strokes() -> Vec<Color32> {
    vec![
        color::field_border(),
        color::accent_700(),
        color::accent(),
        color::accent_400(),
        color::danger_700(),
    ]
}

/// The fill and hairline pairs the standard's states paint: rest, hover, open,
/// on, primary, danger, a lit cell, and a hand-painted ground drawn as a bare
/// fill or a bare hairline. `None` is no hairline.
fn states() -> Vec<(Color32, Option<Color32>)> {
    let none = Color32::TRANSPARENT;
    vec![
        (color::field(), Some(color::field_border())),
        (color::field(), None),
        (color::field(), Some(color::accent_700())),
        (color::hover(), Some(color::field_border())),
        (color::hover(), None),
        (color::hover(), Some(color::accent_700())),
        (color::accent_900(), Some(color::accent_700())),
        (color::accent_900(), None),
        (color::accent_800(), Some(color::accent_700())),
        (color::accent_800(), None),
        (color::accent(), Some(color::accent())),
        (color::accent(), None),
        (color::accent_400(), Some(color::accent_400())),
        (color::accent_400(), None),
        (color::danger_900(), Some(color::danger_700())),
        (color::danger_800(), Some(color::danger_700())),
        (color::text_a(33), None),
        (none, Some(color::field_border())),
        (none, Some(color::accent_700())),
    ]
}

fn uniform(r: CornerRadius) -> Option<u8> {
    (r.nw == r.ne && r.ne == r.sw && r.sw == r.se).then_some(r.nw)
}

/// The opacity `Ui::disable` multiplies a disabled `Ui`'s paint by.
fn disabled_alpha(ctx: &egui::Context) -> f32 {
    ctx.global_style().visuals.disabled_alpha()
}

/// Whether `g` is one of the standard's states faded as a disabled `Ui` fades
/// it (the painter's `gamma_multiply` by `dim`): the Disabled state of a control
/// disabled through its `Ui` rather than by `ui::FieldButton::Disabled`.
fn faded_state(g: &Ground, dim: f32) -> bool {
    let stroke = (g.stroke != Stroke::NONE).then_some(g.stroke.color);
    states()
        .iter()
        .any(|(f, s)| f.gamma_multiply(dim) == g.fill && s.map(|c| c.gamma_multiply(dim)) == stroke)
}

/// What is wrong with `g`, if anything; `dim` is `disabled_alpha`.
fn fault(g: &Ground, dim: f32) -> Option<String> {
    let colour = colour_fault(g);
    if colour.is_some() && faded_state(g, dim) {
        return shape_fault(g);
    }
    shape_fault(g).or(colour)
}

fn colour_fault(g: &Ground) -> Option<String> {
    if !fills().contains(&g.fill) {
        return Some("fill off the standard".into());
    }
    let stroke = (g.stroke != Stroke::NONE).then_some(g.stroke.color);
    if stroke.is_some_and(|c| !strokes().contains(&c)) {
        return Some("hairline off the standard".into());
    }
    if !states().contains(&(g.fill, stroke)) {
        return Some("fill and hairline are no one state".into());
    }
    None
}

fn shape_fault(g: &Ground) -> Option<String> {
    let h = g.rect.height();
    let all = heights();
    let fits: Vec<&Rung> = all.iter().filter(|s| (h - s.h).abs() <= SLOP).collect();
    if fits.is_empty() {
        return Some("height off the standard".into());
    }
    let r = uniform(g.radius);
    if !fits.iter().any(|s| r.is_some_and(|r| s.radii.contains(&r))) {
        return Some(format!(
            "corner off the standard for a {}",
            fits.iter().map(|s| s.what).collect::<Vec<_>>().join("/")
        ));
    }
    None
}

/// A selected 100 × 100 rect with a stroke and a drop shadow, so the Fill, Stroke
/// and Effects cards show rows.
fn with_a_rect(ctx: &egui::Context) -> OndinApp {
    let mut app = OndinApp::headless(ctx);
    let mut ids = ondin_core::IdSource::new(0xA0D);
    let root = ids.mint();
    app.session.adopt_document(Document::new(root), None);
    let id = app.session.ids.mint();
    let parent = app.session.doc.first_page();
    app.session
        .try_commit(Transaction(vec![
            Operation::CreateNode {
                id,
                parent,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Box".into()),
            },
            Operation::SetStrokes {
                id,
                strokes: ondin_core::keyed_by_position([ondin_core::Stroke {
                    width: 2.0,
                    ..Default::default()
                }]),
            },
            Operation::SetEffects {
                id,
                effects: ondin_core::keyed_by_position([ondin_core::Effect::new(
                    ondin_core::EffectKind::DropShadow(Default::default()),
                )]),
            },
        ]))
        .expect("a stroked, shadowed rect");
    app.session.selection.set_one(id);
    app
}

fn text_kind(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::default(),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn with_a_text(ctx: &egui::Context) -> OndinApp {
    let mut app = OndinApp::headless(ctx);
    let mut ids = ondin_core::IdSource::new(0xA0E);
    app.session.adopt_document(Document::new(ids.mint()), None);
    let id = app.session.ids.mint();
    let parent = app.session.doc.first_page();
    app.session
        .try_commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: text_kind("Billed yearly"),
            transform: None,
            name: Some("Caption".into()),
        }]))
        .expect("a text layer");
    app.session.selection.set_one(id);
    app
}

/// A frame laid out by flex, holding one rect.
fn with_auto_layout(ctx: &egui::Context) -> OndinApp {
    let mut app = OndinApp::headless(ctx);
    let mut ids = ondin_core::IdSource::new(0xA0F);
    app.session.adopt_document(Document::new(ids.mint()), None);
    let ids = &mut app.session.ids;
    let (frame, child): (NodeId, NodeId) = (ids.mint(), ids.mint());
    let parent = app.session.doc.first_page();
    app.session
        .try_commit(Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(300.0, 200.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((40.0, 40.0))),
                name: Some("Stack".into()),
            },
            Operation::SetDisplay {
                id: frame,
                display: Some(ondin_core::container::Display::Flex(Default::default())),
            },
            Operation::CreateNode {
                id: child,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Item".into()),
            },
        ]))
        .expect("a flex frame");
    app.session.selection.set_one(frame);
    app
}

/// A selected component set, `Size: Small, Large` × `State: Default, Hover,
/// Pressed`, two variants at Default, with Hover tied (and Pressed by its name),
/// so the Variants card shows its noted chips and its Element row.
fn with_a_set(ctx: &egui::Context) -> OndinApp {
    use ondin_core::variant::{self, Interaction, VariantProp, VariantSet};
    let mut app = OndinApp::headless(ctx);
    let mut ids = ondin_core::IdSource::new(0xA10);
    app.session.adopt_document(Document::new(ids.mint()), None);
    let ids = &mut app.session.ids;
    let [set, small, large] = [(); 3].map(|_| ids.mint());
    let parent = app.session.doc.first_page();
    let frame = |id, parent, index, y: f64, name: &str| Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(80.0, 30.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((10.0, y))),
        name: Some(name.into()),
    };
    let mut ops = vec![
        frame(set, parent, 0, 0.0, "Button"),
        frame(small, set, 0, 10.0, "Small"),
        frame(large, set, 1, 60.0, "Large"),
    ];
    for v in [small, large] {
        ops.push(Operation::SetComponent {
            id: v,
            component: true,
        });
    }
    ops.push(Operation::SetVariantSet {
        id: set,
        set: Some(VariantSet {
            props: vec![
                VariantProp::new("Size", vec!["Small".into(), "Large".into()]),
                VariantProp::new(
                    "State",
                    vec!["Default".into(), "Hover".into(), "Pressed".into()],
                ),
            ],
            ..Default::default()
        }),
    });
    for (v, size) in [(small, "Small"), (large, "Large")] {
        ops.push(Operation::SetVariant {
            id: v,
            values: vec![size.into(), "Default".into()],
        });
    }
    app.session
        .try_commit(Transaction(ops))
        .expect("a component set");
    let tie = variant::tie_state(&app.session.doc, set, 1, 1, Some(Interaction::Hover));
    app.session
        .try_commit(tie.expect("Hover ties"))
        .expect("tied");
    app.session.selection.set_one(set);
    app
}

/// The centre of the text `label` painted in the inspector's column.
fn painted_at(ctx: &egui::Context, app: &mut OndinApp, label: &str) -> egui::Pos2 {
    let column = OndinApp::inspector_column(Rect::from_min_size(egui::Pos2::ZERO, SCREEN));
    let out = settle(ctx, app);
    let inks = texts(&out);
    inks.iter()
        .find(|t| t.text == label && column.contains(t.rect.center()))
        .map(|t| t.rect.center())
        .unwrap_or_else(|| {
            let all: Vec<&str> = inks.iter().map(|t| t.text.as_str()).collect();
            panic!("no {label:?} in the inspector: {all:?}")
        })
}

/// Open the popup menu hanging from the clickable widget at `at` — the last
/// registered there on the topmost layer, the one a click reaches — as its click
/// would (`egui::Popup::menu` keys it `Popup::default_response_id`), the
/// pointer left on the widget.
fn open_popup_at(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
    pass(ctx, app, vec![egui::Event::PointerMoved(at)]);
    settle(ctx, app);
    // Read the layer first: Context accessors never nest. Layers are a map, so
    // only one layer's widgets come in an order.
    let layer = ctx.layer_id_at(at).expect("a layer under the point");
    let id = ctx.viewport(|v| {
        v.this_pass
            .widgets
            .get_layer(layer)
            .filter(|w| w.rect.contains(at) && w.sense.senses_click())
            .last()
            .map(|w| w.id)
    });
    egui::Popup::open_id(ctx, id.expect("a clickable widget").with("popup"));
    settle(ctx, app);
}

/// What `open` brings up, as [`opened`] finds it, and apart from it the
/// grounds its list rows paint under the pointer: each control `open` added,
/// hovered in turn. A menu row paints no ground at rest, so only a sweep sees it.
fn swept(
    ctx: &egui::Context,
    app: &mut OndinApp,
    open: impl FnOnce(&egui::Context, &mut OndinApp),
) -> (Vec<Ground>, Vec<Ground>) {
    settle(ctx, app);
    let before: Vec<Rect> = controls(ctx).iter().map(|c| c.rect).collect();
    // A row `open` left lit (the one a flyout opens from) is a list row too. By
    // its missing hairline as well as its fill: a control face hovered under the
    // pointer that opened the popup wears the same hover ground.
    let (mut rows, resting): (Vec<Ground>, Vec<Ground>) = opened(ctx, app, open)
        .into_iter()
        .partition(|g| g.fill == list_hover() && g.stroke == Stroke::NONE);
    let added: Vec<Control> = controls(ctx)
        .into_iter()
        .filter(|c| c.enabled && !c.drags && !before.contains(&c.rect))
        .collect();
    for c in added {
        pass(ctx, app, vec![egui::Event::PointerMoved(c.rect.center())]);
        pass(ctx, app, Vec::new());
        let out = pass(ctx, app, Vec::new());
        let under = grounds(ctx, &out)
            .into_iter()
            .find(|g| holds(g.rect, c.rect) == Some(Holds::Whole));
        if let Some(g) = under
            && !resting.iter().chain(&rows).any(|r| r.rect == g.rect)
        {
            rows.push(g);
        }
    }
    (resting, rows)
}

/// The fill `ui::menu_row` paints a list row under the pointer, its only ground:
/// the one hover ground, as a dropdown's list rows and `ui::list_row_ground` use.
fn list_hover() -> Color32 {
    color::hover()
}

/// What is wrong with a list row's ground under the pointer, if anything: it
/// must be a list row's height and corner, in the list hover and no hairline.
fn row_fault(g: &Ground) -> Option<String> {
    let rung = (g.rect.height() - ui::MENU_ROW_H).abs() <= SLOP && uniform(g.radius) == Some(4);
    if !rung {
        return Some("not a list row's height and corner".into());
    }
    (g.fill != list_hover() || g.stroke != Stroke::NONE).then(|| "not the list row's hover".into())
}

/// The control grounds in the inspector's column.
fn inspector(ctx: &egui::Context, app: &mut OndinApp) -> Vec<Ground> {
    let column = OndinApp::inspector_column(Rect::from_min_size(egui::Pos2::ZERO, SCREEN));
    let out = settle(ctx, app);
    grounds(ctx, &out)
        .into_iter()
        .filter(|g| column.contains(g.rect.center()))
        .collect()
}

/// The control grounds `open` brings up: those painted after it that were not
/// painted, with the same paint, before it.
fn opened(
    ctx: &egui::Context,
    app: &mut OndinApp,
    open: impl FnOnce(&egui::Context, &mut OndinApp),
) -> Vec<Ground> {
    let before = grounds(ctx, &settle(ctx, app));
    open(ctx, app);
    let after = grounds(ctx, &settle(ctx, app));
    let was = |g: &Ground| {
        before
            .iter()
            .any(|b| b.rect == g.rect && b.fill == g.fill && b.stroke == g.stroke)
    };
    after.into_iter().filter(|g| !was(g)).collect()
}

/// The fills a control may take under the pointer: the hover ground, or the
/// next ramp step of a filled state.
fn hover_fills() -> Vec<Color32> {
    vec![
        color::hover(),
        color::accent_800(),
        color::accent_400(),
        color::danger_800(),
    ]
}

/// Puts the pointer on each live ground of `rest` in turn and files what it
/// turns into: a hover fill, its hairline unchanged unless it was already the
/// open state's. A lit segment cell stays lit and a field keeps its ground.
/// Returns how many it hovered.
fn hover(
    ctx: &egui::Context,
    app: &mut OndinApp,
    name: &str,
    rest: &[Ground],
    faults: &mut Vec<String>,
) -> usize {
    let mut n = 0;
    for g in rest
        .iter()
        .filter(|g| g.live && !EXEMPT.iter().any(|e| (e.is)(g)))
    {
        pass(ctx, app, vec![egui::Event::PointerMoved(g.rect.center())]);
        pass(ctx, app, Vec::new());
        let out = pass(ctx, app, Vec::new());
        n += 1;
        let Some(mut h) = grounds(ctx, &out).into_iter().find(|h| h.rect == g.rect) else {
            faults.push(format!("{name}: no ground under the pointer: {g}"));
            continue;
        };
        // A hover painted just inside the hairline, concentric with it, reads as
        // the ground's own (an effect row lays it under its contents that way).
        let inset = rect_shapes(&out).into_iter().find(|(_, s)| {
            s.rect == g.rect.shrink(1.0)
                && uniform(s.corner_radius).zip(uniform(g.radius))
                    == Some((ui::BUTTON_R - 1, ui::BUTTON_R))
                && s.fill != Color32::TRANSPARENT
        });
        if let Some((_, s)) = inset {
            h.fill = s.fill;
        }
        // A lit cell stays lit; a field keeps its ground (its states are rest
        // and typing focus).
        let kept = (g.fill == color::text_a(33) || g.field) && h.fill == g.fill;
        if !kept && !hover_fills().contains(&h.fill) {
            faults.push(format!(
                "{name}: hover fill off the standard: {h} (at rest {g})"
            ));
        } else if h.stroke.color == color::accent_700() && g.stroke.color != color::accent_700() {
            faults.push(format!("{name}: hover lit the hairline: {h} (at rest {g})"));
        } else if let Some(why) = fault(&h, disabled_alpha(ctx)) {
            faults.push(format!("{name}: under the pointer, {why}: {h}"));
        }
    }
    pass(ctx, app, vec![egui::Event::PointerGone]);
    n
}

/// Files every fault among `found` under the surface `name`; returns how many
/// grounds it checked.
fn audit(name: &str, found: &[Ground], dim: f32, faults: &mut Vec<String>) -> usize {
    let dump = std::env::var_os("ONDIN_AUDIT_DUMP").is_some();
    for g in found {
        if dump {
            eprintln!("{name}: {g} live={} field={}", g.live, g.field);
        }
        if let Some(e) = EXEMPT.iter().find(|e| (e.is)(g)) {
            if dump {
                eprintln!("  exempt, {}: {}", e.what, e.why);
            }
            continue;
        }
        if let Some(why) = fault(g, dim) {
            faults.push(format!("{name}: {why}: {g}"));
        }
    }
    found.len()
}

/// Every control ground on each reachable surface keeps the form-control
/// standard's height, corner, fill and hairline, at rest and under the pointer.
/// A control disabled through its `Ui` fades as a whole: any standard state at
/// egui's `disabled_alpha` is the standard's Disabled under a disabled `Ui`.
///
/// flip: `ui::button_face` rounding to 4 → fails at the faults assertion, a
/// "corner off the standard" for every field button.
#[test]
fn every_control_ground_is_on_the_standard() {
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut faults = Vec::new();
    let mut seen = Vec::new();
    let mut check = |name: &'static str, mut app: OndinApp, found: Vec<Ground>| {
        let at_rest = audit(name, &found, disabled_alpha(&ctx), &mut faults);
        let hovered = hover(&ctx, &mut app, name, &found, &mut faults);
        seen.push((name, at_rest, hovered));
    };

    let mut app = with_a_rect(&ctx);
    let found = inspector(&ctx, &mut app);
    check("rect inspector", app, found);
    let mut app = with_a_text(&ctx);
    let found = inspector(&ctx, &mut app);
    check("text inspector", app, found);
    let mut app = with_auto_layout(&ctx);
    let found = inspector(&ctx, &mut app);
    check("auto layout", app, found);

    let mut app = with_a_rect(&ctx);
    let found = opened(&ctx, &mut app, |_, a| a.stroke_menu = Some(0));
    check("stroke popover", app, found);
    let mut app = with_a_rect(&ctx);
    let found = opened(&ctx, &mut app, |_, a| {
        let id = a.session.selection.ids()[0];
        a.effect_menu = Some((id, 0));
    });
    check("effect popover", app, found);
    let mut app = with_a_text(&ctx);
    let found = opened(&ctx, &mut app, |_, a| {
        a.type_menu = Some(crate::app::TypeTab::Character)
    });
    check("type popover", app, found);
    let mut app = with_a_rect(&ctx);
    let found = opened(&ctx, &mut app, |c, a| a.toggle_settings(c));
    check("settings", app, found);
    let mut app = with_a_text(&ctx);
    let found = opened(&ctx, &mut app, |_, a| a.open_find(true));
    check("find", app, found);
    let mut app = with_a_rect(&ctx);
    let found = opened(&ctx, &mut app, |c, a| a.open_batch_rename(c, None));
    check("batch rename", app, found);
    let mut app = with_a_rect(&ctx);
    let found = opened(&ctx, &mut app, |_, a| {
        // A colour a hair from the rect's fill, so a group shows unfolded.
        let id = a.session.selection.ids()[0];
        let fill = |r, g, b| {
            ondin_core::keyed_by_position([ondin_core::Fill {
                brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::from_rgb8(r, g, b))
                    .into(),
                visible: true,
            }])
        };
        let twin = a.session.ids.mint();
        let page = a.session.page;
        assert!(a.session.commit(Transaction(vec![
            Operation::SetFills {
                id,
                fills: fill(1, 1, 2),
            },
            Operation::CreateNode {
                id: twin,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetFills {
                id: twin,
                fills: fill(1, 1, 1),
            },
        ])));
        // On the document: nothing selected.
        a.session.selection.set(Vec::new());
        a.open_colour_review();
    });
    assert_eq!(app.colour_review.groups_shown(), 1, "a group unfolded");
    check("colour review", app, found);
    let mut app = with_a_set(&ctx);
    let found = inspector(&ctx, &mut app);
    check("variants card", app, found);

    // A value chip's popup, then its *State ▸* flyout over it: list rows, whose
    // ground is the hover alone, so each is swept.
    let mut rows_seen: Vec<(&str, usize)> = Vec::new();
    let mut rows_audit = |name: &'static str, rows: &[Ground], faults: &mut Vec<String>| {
        for g in rows {
            if std::env::var_os("ONDIN_AUDIT_DUMP").is_some() {
                eprintln!("{name} row: {g}");
            }
            if let Some(why) = row_fault(g) {
                faults.push(format!("{name}: {why}: {g}"));
            }
        }
        rows_seen.push((name, rows.len()));
    };
    let chip = |c: &egui::Context, a: &mut OndinApp| {
        let at = painted_at(c, a, "Hover");
        open_popup_at(c, a, at);
    };
    let mut app = with_a_set(&ctx);
    let (resting, popup_rows) = swept(&ctx, &mut app, chip);
    audit(
        "value chip popup",
        &resting,
        disabled_alpha(&ctx),
        &mut faults,
    );
    hover(&ctx, &mut app, "value chip popup", &resting, &mut faults);
    rows_audit("value chip popup", &popup_rows, &mut faults);
    let mut app = with_a_set(&ctx);
    let below = painted_at(&ctx, &mut app, "Hover").y;
    chip(&ctx, &mut app);
    let out = pass(&ctx, &mut app, Vec::new());
    let menu = texts(&out).into_iter().any(|t| t.text == "Move earlier");
    assert!(menu, "the chip's popup is open");
    let (resting, rows) = swept(&ctx, &mut app, |c, a| {
        let out = settle(c, a);
        let state = texts(&out)
            .into_iter()
            .find(|t| t.text == "State" && t.rect.center().y > below)
            .unwrap_or_else(|| {
                let all: Vec<(String, f32)> = texts(&out)
                    .into_iter()
                    .map(|t| (t.text, t.rect.center().y))
                    .collect();
                panic!("the chip popup's State row, below {below}: {all:?}")
            });
        pass(c, a, vec![egui::Event::PointerMoved(state.rect.center())]);
    });
    audit("state flyout", &resting, disabled_alpha(&ctx), &mut faults);
    rows_audit("state flyout", &rows, &mut faults);
    // A list beside a menu's row is that menu's width.
    let popup_w = popup_rows.first().map(|g| g.rect.width());
    for g in rows.iter().filter(|g| Some(g.rect.width()) != popup_w) {
        faults.push(format!(
            "state flyout: a row {:.1} wide beside the popup's {popup_w:?}: {g}",
            g.rect.width()
        ));
    }

    for (name, n) in &rows_seen {
        assert!(
            *n > 2,
            "{name}: the fixture is not in the state it is about — it swept {n} list rows"
        );
    }
    for (name, at_rest, hovered) in &seen {
        assert!(
            *at_rest > 2 && *hovered > 0,
            "{name}: the fixture is not in the state it is about — it painted \
             {at_rest} control grounds and hovered {hovered}"
        );
    }
    assert!(
        faults.is_empty(),
        "controls off the form-control standard:\n  {}",
        faults.join("\n  ")
    );
}
