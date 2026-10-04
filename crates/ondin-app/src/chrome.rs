//! **The window's own chrome** (§15 D952): the caption buttons, the strip a
//! window is dragged by, and the edges it is resized from — drawn by the app on
//! the hosts where the system's are turned off.
//!
//! **One question, asked of one value.** Which host draws what is decided once,
//! in [`Host::current`], and everything else asks a [`Chrome`] a capability —
//! *does the app draw its own buttons?*, *how far must the bar's content stay
//! from the left edge?* — rather than testing the target at the use site. The
//! rule is Schemaic's (`schemaic-core`'s `window_chrome`), and it is worth more
//! here than there: the four decision functions CLAUDE.md lists as never compiled
//! on this machine are each `cfg`-gated at their item, and a capability keeps
//! every branch compiled on every host. `cfg!` appears in this file once.
//!
//! **What each host keeps:**
//!
//! - **Windows** — no decorations. winit then drops the caption *and* the sizing
//!   frame, so without [`resize_zones`] the window could not be resized at all;
//!   egui-winit keeps the DWM drop shadow on an undecorated window by itself.
//!   Aero Snap still works, because a drag begun by [`egui::ViewportCommand::StartDrag`]
//!   is the system's own caption drag.
//! - **Linux** — the same, X11 and Wayland alike. Wayland has no server-side
//!   shadow to ask for, and the app id is set so the window matches its desktop
//!   entry.
//! - **macOS** — decorations kept and the title bar made transparent over a
//!   full-size content view, so the **native traffic lights and resize border
//!   stay** and the app's own bar runs underneath them. The app draws no buttons
//!   and no resize zones there, and keeps its content clear of the lights.

use egui::{Color32, CursorIcon, Id, Order, Pos2, Rect, ResizeDirection, Sense, Stroke, Vec2};

use crate::theme::color;

/// The reverse-DNS id the app carries everywhere it needs one: the Wayland app
/// id and X11 `WM_CLASS` here, the desktop entry and AppStream metadata in
/// `packaging/linux/`, the macOS bundle id. One identity on every platform.
pub(crate) const APP_ID: &str = "io.github.fadion.Ondin";

/// A caption button's width — Windows 11's caption metric. Its height is the
/// bar's.
pub(crate) const CONTROL_W: f32 = 46.0;

/// How far the resize edge reaches into the window, in points — about the
/// width of the system frame it stands in for.
const EDGE: f32 = 5.0;

/// A corner zone's side. Larger than an edge, so a corner is easy to hit, and
/// each edge stops this short of the corner so the two never contest a point.
const CORNER: f32 = 14.0;

/// The traffic lights' room on macOS: three lights from the window's left edge,
/// about 52 points wide at the system's offset, plus a gap.
const MAC_LIGHTS: f32 = 72.0;

/// What the chrome senses: the pointer, and **never the keyboard focus**. The
/// system's caption, buttons and frame are not in a window's Tab order, and
/// these stand in for them; `Sense::click_and_drag` would add `FOCUSABLE`, and
/// then the first `Tab` in the app landed on the drag strip — which both top
/// bars draw under one id, so the focus outlived a switch of screen and
/// `chrome_focus` reported a field nobody could see (caught by
/// `chrome_focus_write_tests`).
const POINTER_ONLY: Sense = Sense::CLICK.union(Sense::DRAG);

/// The close button's hover fill — Windows' own caption red, under a white
/// glyph, which is what every Windows user reads as "this closes the window".
const CLOSE_HOVER: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);

/// The height of present mode's strip ([`present_strip`]) — Windows 11's own
/// caption height, so its buttons are the system's shape rather than the top
/// bar's taller one.
const STRIP_H: f32 = 32.0;

/// How close to the window's top edge the pointer must come to bring present
/// mode's strip out. A band, not the single top row: a pointer thrown at the
/// top of a maximized window lands on row 0, but one moved there by hand on a
/// window that is not maximized stops wherever the hand does.
const REVEAL: f32 = 6.0;

/// Which kind of window manager the app is running under. BSDs and anything
/// else unrecognised are treated as Linux: an X11 or Wayland desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    Windows,
    MacOs,
    Linux,
}

impl Host {
    /// The host this binary was built for — the one `cfg!` in this module.
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Host::MacOs
        } else if cfg!(windows) {
            Host::Windows
        } else {
            Host::Linux
        }
    }
}

/// What the window leaves to the app on one host. See the module doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chrome {
    host: Host,
}

impl Chrome {
    pub(crate) fn current() -> Self {
        Self::of(Host::current())
    }

    pub(crate) fn of(host: Host) -> Self {
        Self { host }
    }

    /// Whether the app draws minimize, maximize and close itself. Everywhere
    /// but macOS, whose traffic lights are kept.
    pub(crate) fn draws_own_controls(self) -> bool {
        self.host != Host::MacOs
    }

    /// Whether the app supplies the edges a window is resized from. The same
    /// hosts: an undecorated window has no sizing frame, and macOS keeps its own.
    pub(crate) fn draws_own_resize_border(self) -> bool {
        self.host != Host::MacOs
    }

    /// How far a top bar's content must start from the window's left edge to
    /// clear what the system draws there: the traffic lights on macOS, nothing
    /// elsewhere.
    pub(crate) fn leading_inset(self) -> f32 {
        match self.host {
            Host::MacOs => MAC_LIGHTS,
            Host::Windows | Host::Linux => 0.0,
        }
    }

    /// The window as this host should open it.
    pub(crate) fn viewport(self, builder: egui::ViewportBuilder) -> egui::ViewportBuilder {
        let builder = builder.with_app_id(APP_ID);
        match self.host {
            Host::MacOs => builder
                .with_fullsize_content_view(true)
                .with_titlebar_shown(false)
                .with_title_shown(false),
            Host::Windows | Host::Linux => builder.with_decorations(false),
        }
    }
}

/// **A top bar's frame**: `fill`, `margin` points of padding on the left plus
/// whatever the host draws there ([`Chrome::leading_inset`]), and **none on the
/// right** — the caption buttons sit flush against the window's edge, and the
/// bar's own right margin is put back after them by [`after_buttons`].
pub(crate) fn bar_frame(fill: Color32, margin: f32, chrome: Chrome) -> egui::Frame {
    // Points, as `i8`: a 14-point margin and a 72-point inset are well inside it.
    let left = (margin + chrome.leading_inset()).round() as i8;
    egui::Frame::NONE.fill(fill).inner_margin(egui::Margin {
        left,
        right: 0,
        top: 0,
        bottom: 0,
    })
}

/// The whole of a top bar's rectangle, from inside the [`bar_frame`] it was
/// shown in — its content rectangle widened back over the left padding — for
/// [`drag_strip`].
pub(crate) fn bar_rect(ui: &egui::Ui, margin: f32, chrome: Chrome) -> Rect {
    let inner = ui.max_rect();
    Rect::from_min_max(
        Pos2::new(inner.min.x - margin - chrome.leading_inset(), inner.min.y),
        inner.max,
    )
}

/// The bar's right margin, put back between the caption buttons and the bar's
/// own content in a `right_to_left` row: `margin` in all, of which the row's
/// `item_spacing` already supplies `gap`.
pub(crate) fn after_buttons(ui: &mut egui::Ui, margin: f32, gap: f32) {
    ui.add_space((margin - gap).max(0.0));
}

/// Whether the window is maximized now, as the last frame's viewport reported
/// it. **Read, not remembered**: a snap, Win+↑ or a drag off a maximized
/// window all change it without passing through a button here.
fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// **The strip a top bar is dragged by**, laid behind the bar's own widgets:
/// call it first in the bar, with the bar's whole rectangle. A press-drag on
/// any part of the bar that no widget answers starts the system's move loop,
/// and a double-click maximizes or restores — on every host, macOS included,
/// where the bar runs under the transparent title bar.
///
/// **First, so it is underneath.** egui gives a press to the last widget
/// registered under the pointer, so everything the bar adds after this —
/// buttons, the breadcrumb, menus — keeps its own clicks, and only the empty
/// parts and plain labels (which sense hover, not clicks, since labels are not
/// selectable here) fall through to the strip.
///
/// ⚠️ **Underneath is not enough for a drag, and the strip arms itself on the
/// press.** egui's hit test hands a *click*-only widget the click and the large
/// drag-sensing thing beneath it the drag — the shape of a button over a scroll
/// area — so a press on a bar button that then moved dragged the window, which
/// no system caption does. So the press arms the strip only when nothing else
/// interactive is under the pointer (`armed`), and a drag starts the window's
/// move only from an armed press.
///
/// **`act` false makes it inert for the frame** — sensing, so it still stands
/// under the bar's widgets, and sending nothing. The library passes
/// `!menu_was_up`: a click that dismisses one of its menus is spent doing so
/// (§15 D558, D973), and a double-click that closed a menu with its first half
/// would otherwise maximize the window with its second.
pub(crate) fn drag_strip(ui: &mut egui::Ui, rect: Rect, act: bool) {
    strip(ui, rect, act, Id::new("window-drag-strip"));
}

/// [`drag_strip`] under `id` — its own, or [`above_modal`]'s, which lays a
/// second strip in the same pass and must not clash with the first.
fn strip(ui: &mut egui::Ui, rect: Rect, act: bool, id: Id) {
    let strip = ui.interact(rect, id, POINTER_ONLY);
    let ctx = ui.ctx().clone();
    // Where this pass's bar is, for `above_modal`, which lays the same strip
    // and buttons again over a card's backdrop. Read into a local first: two
    // `Context` accessors nested deadlock (CLAUDE.md).
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(bar_key(), (rect, pass)));
    // The last press, as `(armed, spent, the press before it was spent)`:
    // armed when it was on the strip over nothing else that answers a press,
    // spent when `act` was false.
    let press_key = id.with("press");
    let pressed = ctx.input(|i| i.pointer.primary_pressed());
    if pressed && (strip.contains_pointer() || !act) {
        let hovered: Vec<Id> = ctx.interaction_snapshot(|s| s.hovered.iter().copied().collect());
        let other = hovered.into_iter().any(|h| {
            h != id
                && ctx
                    .read_response(h)
                    .is_some_and(|r| r.sense.senses_click() || r.sense.senses_drag())
        });
        let (_, spent, _) = ctx.data(|d| {
            d.get_temp::<(bool, bool, bool)>(press_key)
                .unwrap_or_default()
        });
        ctx.data_mut(|d| d.insert_temp(press_key, (act && !other, !act, spent)));
    }
    if !act {
        return;
    }
    let (armed, spent, spent_before) = ctx.data(|d| {
        d.get_temp::<(bool, bool, bool)>(press_key)
            .unwrap_or_default()
    });
    // ⚠️ **Neither press of a double-click spent**, not only the second: the
    // first may be the one a menu was dismissed by, and then the pair is not a
    // double-click on the bar but a dismissal and a click.
    if strip.double_clicked() {
        if !spent && !spent_before {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized(&ctx)));
        }
    } else if strip.drag_started_by(egui::PointerButton::Primary) && armed {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// The key `drag_strip` notes its rectangle under, with the pass it was laid
/// in.
fn bar_key() -> Id {
    Id::new("window-drag-strip").with("rect")
}

/// The key `settings::card` notes a card's rectangle under ([`note_card`]).
fn card_key() -> Id {
    Id::new("chrome-modal-card")
}

/// Note that a modal card was drawn this pass, at `rect`, for [`above_modal`].
/// `settings::card` calls it, which every card in the app goes through.
pub(crate) fn note_card(ctx: &egui::Context, rect: Rect) {
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|d| d.insert_temp(card_key(), (rect, pass)));
}

/// **The window's own controls, over a modal card's backdrop** (§15 D973,
/// `[X2-L1-02]`). Call once a frame, after every card.
///
/// `egui::Modal` refuses interaction to every layer below its own, and the
/// top bar is a panel and the resize zones older areas — so while any card
/// was up, the caption buttons, the drag strip and the resize edges were dead,
/// and the mouse could not move, resize, minimize, maximize or close the
/// window. The system's frame, which these stand in for, is never blocked by an
/// app's own dialog. So with a card drawn this pass the zones are raised above
/// it, and the bar's strip and buttons are laid again in an area of their own
/// on top — **unless the card reaches into the bar**, on a window short enough
/// for a tall card to, where the card's own top edge wins.
///
/// **Close is still a close request**, so with *Unsaved changes* up a click on
/// ✕ re-arms that same card rather than closing anything
/// (`OndinApp::handle_close_request`).
pub(crate) fn above_modal(ctx: &egui::Context, chrome: Chrome) {
    let pass = ctx.cumulative_pass_nr();
    let card = ctx.data(|d| d.get_temp::<(Rect, u64)>(card_key()));
    let Some((card, _)) = card.filter(|&(_, at)| at == pass) else {
        return;
    };
    let bar = ctx.data(|d| d.get_temp::<(Rect, u64)>(bar_key()));
    let bar = bar.filter(|&(r, at)| at == pass && !r.intersects(card));
    if let (true, Some((bar, _))) = (chrome.draws_own_controls(), bar) {
        let id = Id::new("chrome-above-modal");
        egui::Area::new(id)
            .order(Order::Foreground)
            .fixed_pos(bar.min)
            .constrain(false)
            .show(ctx, |ui| {
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(bar)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        strip(ui, bar, true, id.with("strip"));
                        caption_buttons(ui, chrome, true);
                    },
                );
                // ⚠️ **The zones' share of the bar, again, in this area and
                // after the strip and buttons**, so they win there as they do
                // with no card. Laid only in the separate zone areas, the
                // lifted strip covered the top edge, both top corners and the
                // sides down to the bar's height, and a press there moved the
                // window instead of resizing it (found by `arch-scribe` reading
                // this, §15 D973). Raising the zone areas *after* this one does
                // not fix it: egui keeps the layers asking to be on top in a
                // hash set and sorts stably, so among them the order is their
                // age, never the order of the `move_to_top` calls — measured,
                // the first repair of this was green in no test.
                if chrome.draws_own_resize_border() && !maximized(ctx) {
                    for (i, (direction, rect)) in zones(ctx.content_rect()).into_iter().enumerate()
                    {
                        if rect.intersects(bar) {
                            let resp = ui.interact(rect, id.with(("zone", i)), POINTER_ONLY);
                            zone_acts(ui, resp, direction);
                        }
                    }
                }
            });
        ctx.move_to_top(egui::LayerId::new(Order::Foreground, id));
    }
    // And every zone above the card, which is what lets the edges below the
    // bar resize the window at all with a card up.
    if chrome.draws_own_resize_border() && !maximized(ctx) {
        for (i, (direction, _)) in zones(ctx.content_rect()).iter().enumerate() {
            ctx.move_to_top(egui::LayerId::new(
                Order::Foreground,
                zone_id(i, *direction),
            ));
        }
    }
}

/// **Minimize, maximize or restore, and close**, for the right end of a top
/// bar laid out `right_to_left` — so they are added close-first, and read
/// minimize, maximize, close on screen. Each is the bar's full height and
/// [`CONTROL_W`] wide, flush against the window's right edge, with no margin
/// beyond it: the corner is where a pointer thrown at "close" lands.
///
/// Nothing on a host that keeps its own ([`Chrome::draws_own_controls`]).
///
/// **Close sends [`egui::ViewportCommand::Close`]**, which is the same request
/// the system's button makes, so it passes through `handle_close_request` and
/// unsaved work is asked about exactly as it is for Alt+F4.
///
/// **`act` false draws them and sends nothing** — [`drag_strip`]'s `act`, for
/// the same click that dismisses a menu (§15 D973, `[R1-L2-02]`).
pub(crate) fn caption_buttons(ui: &mut egui::Ui, chrome: Chrome, act: bool) {
    if !chrome.draws_own_controls() {
        return;
    }
    // Touching, as the system's are: in a scope of their own, so the bar's item
    // spacing neither separates them nor is changed for what follows.
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        three_buttons(ui, act);
    });
}

fn three_buttons(ui: &mut egui::Ui, act: bool) {
    let ctx = ui.ctx().clone();
    let height = ui.available_height();
    let glyph = |ui: &egui::Ui, rect: Rect, hovered: bool, close: bool| {
        let fg = match (hovered, close) {
            (true, true) => Color32::WHITE,
            (true, false) => color::TEXT,
            (false, _) => crate::theme::text::MUTED,
        };
        (
            ui.painter().clone(),
            Stroke::new(1.0, fg),
            snap(rect.center()),
        )
    };
    let button = |ui: &mut egui::Ui, close: bool| {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(CONTROL_W, height), Sense::CLICK);
        if resp.hovered() {
            let fill = if close { CLOSE_HOVER } else { color::HOVER };
            ui.painter().rect_filled(rect, 0.0, fill);
        }
        (rect, resp)
    };

    // Close, rightmost.
    let (rect, resp) = button(ui, true);
    let (p, stroke, c) = glyph(ui, rect, resp.hovered(), true);
    let h = 5.0;
    p.line_segment([c + Vec2::new(-h, -h), c + Vec2::new(h, h)], stroke);
    p.line_segment([c + Vec2::new(-h, h), c + Vec2::new(h, -h)], stroke);
    if resp.on_hover_text("Close").clicked() && act {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    // Maximize, or restore while maximized: one square, or two offset ones.
    let max = maximized(&ctx);
    let (rect, resp) = button(ui, false);
    let (p, stroke, c) = glyph(ui, rect, resp.hovered(), false);
    if max {
        let front = Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(8.0));
        p.rect_stroke(front, 0.0, stroke, egui::StrokeKind::Middle);
        let back = [
            Pos2::new(front.min.x + 2.0, front.min.y - 2.0),
            Pos2::new(front.max.x + 2.0, front.min.y - 2.0),
            Pos2::new(front.max.x + 2.0, front.max.y - 2.0),
        ];
        p.line_segment([back[0], back[1]], stroke);
        p.line_segment([back[1], back[2]], stroke);
    } else {
        let square = Rect::from_center_size(c, Vec2::splat(10.0));
        p.rect_stroke(square, 0.0, stroke, egui::StrokeKind::Middle);
    }
    if resp
        .on_hover_text(if max { "Restore" } else { "Maximize" })
        .clicked()
        && act
    {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }

    // Minimize, leftmost of the three.
    let (rect, resp) = button(ui, false);
    let (p, stroke, c) = glyph(ui, rect, resp.hovered(), false);
    p.line_segment([c + Vec2::new(-5.0, 0.0), c + Vec2::new(5.0, 0.0)], stroke);
    if resp.on_hover_text("Minimize").clicked() && act {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
}

/// **Present mode's way to the window's buttons** (§15 D958): a slim strip
/// across the top of the window — the drag strip and the three caption buttons
/// over the top bar's fill — shown while the pointer is at the window's top
/// edge and for as long as it stays on the strip. Call once a frame while
/// present mode is on.
///
/// Present mode hides the top bar, and on a host that draws its own controls
/// the top bar *is* the title bar: without this the window could not be
/// closed, moved, maximized or minimized with the mouse until Escape. Nothing
/// on macOS ([`Chrome::draws_own_controls`]), whose traffic lights are the
/// system's and stay on screen.
///
/// **Revealed only from the edge, and only with no button held.** Coming back
/// down to the strip's height from below shows nothing — the canvas under it
/// is artwork somebody is working on — and a drag that carries a shape up to
/// the top of the screen must not open a strip over it. Once out, a held
/// button keeps it out, so a press on a caption button that strays off the
/// strip before the release does not take the button away from under it.
///
/// ⚠️ **[`Order::Middle`], not [`Order::Foreground`]**, which is the layer the
/// resize zones are on ([`resize_zones`]). The strip is above the canvas and
/// below the zones, as the top bar is: with the strip on top, the edge that
/// reveals it would have been the edge the window could no longer be resized
/// from, since reaching it is what puts the strip there.
pub(crate) fn present_strip(ctx: &egui::Context, chrome: Chrome) {
    if !chrome.draws_own_controls() {
        return;
    }
    let id = Id::new("present-strip");
    let screen = ctx.content_rect();
    let strip = Rect::from_min_size(screen.min, Vec2::new(screen.width(), STRIP_H));
    let (pointer, down) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.any_down()));
    let was = ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
    let shown = match pointer {
        Some(p) if was => down || strip.contains(p),
        Some(p) => !down && p.y <= screen.min.y + REVEAL,
        None => was && down,
    };
    ctx.data_mut(|d| d.insert_temp(id, shown));
    if !shown {
        return;
    }
    egui::Area::new(id)
        .order(Order::Middle)
        .fixed_pos(strip.min)
        .constrain(false)
        .show(ctx, |ui| {
            // The whole strip is the area's, so the canvas never sees a pointer
            // that is over any part of it.
            ui.set_min_size(strip.size());
            ui.painter().rect_filled(strip, 0.0, color::TOPBAR);
            ui.painter().hline(
                strip.x_range(),
                strip.max.y - 0.5,
                Stroke::new(1.0, color::DIVIDER),
            );
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(strip)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    drag_strip(ui, strip, true);
                    caption_buttons(ui, chrome, true);
                },
            );
        });
}

/// A point on the half-pixel, so a 1-point stroke through it lands on one row
/// of device pixels at 100% rather than smearing across two.
fn snap(p: Pos2) -> Pos2 {
    Pos2::new(p.x.floor() + 0.5, p.y.floor() + 0.5)
}

/// The zones a window is resized from, as `(direction, rect)` within the
/// window's `screen`: four edges [`EDGE`] deep, each stopping [`CORNER`] short of
/// its ends, and four corners, each an **L** of two [`EDGE`]-deep arms
/// [`CORNER`] long.
///
/// ⚠️ **Two rectangles per corner, because an L is not a rectangle** (§15 D973,
/// `[X2-L3-01]`). Each corner was written `a.union(b)` — read as an L, and the
/// tests' prose said so — but `Rect::union` is the bounding box, so every
/// corner was a full 14-point square and its inner 9×9 resized the window: a
/// press 7 points in from the top-right corner, on the close button, began a
/// north-east resize. Ruled an L under the maintainer's delegation: the corner
/// a thrown pointer lands on is the close button's, and the thickness of the
/// frame it stands in for is [`EDGE`], not [`CORNER`].
fn zones(screen: Rect) -> [(ResizeDirection, Rect); 12] {
    let (l, t, r, b) = (screen.min.x, screen.min.y, screen.max.x, screen.max.y);
    let rect = |x0: f32, y0: f32, x1: f32, y1: f32| {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    };
    // Each corner's horizontal arm takes the square where the arms meet, and
    // its vertical arm starts below it, so no point is in both.
    use ResizeDirection::{NorthEast, NorthWest, SouthEast, SouthWest};
    [
        (
            ResizeDirection::North,
            rect(l + CORNER, t, r - CORNER, t + EDGE),
        ),
        (
            ResizeDirection::South,
            rect(l + CORNER, b - EDGE, r - CORNER, b),
        ),
        (
            ResizeDirection::West,
            rect(l, t + CORNER, l + EDGE, b - CORNER),
        ),
        (
            ResizeDirection::East,
            rect(r - EDGE, t + CORNER, r, b - CORNER),
        ),
        (NorthWest, rect(l, t, l + CORNER, t + EDGE)),
        (NorthWest, rect(l, t + EDGE, l + EDGE, t + CORNER)),
        (NorthEast, rect(r - CORNER, t, r, t + EDGE)),
        (NorthEast, rect(r - EDGE, t + EDGE, r, t + CORNER)),
        (SouthWest, rect(l, b - EDGE, l + CORNER, b)),
        (SouthWest, rect(l, b - CORNER, l + EDGE, b - EDGE)),
        (SouthEast, rect(r - CORNER, b - EDGE, r, b)),
        (SouthEast, rect(r - EDGE, b - CORNER, r, b - EDGE)),
    ]
}

/// The area id of the `i`th of [`zones`] — by index as well as direction,
/// since each corner direction names two areas.
fn zone_id(i: usize, direction: ResizeDirection) -> Id {
    Id::new(("window-resize", i, format!("{direction:?}")))
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
    }
}

/// **The edges and corners the window is resized from**, on a host whose
/// window has no sizing frame of its own ([`Chrome::draws_own_resize_border`]).
/// Call once a frame, from either screen.
///
/// Each zone is an [`egui::Area`] of its own on [`Order::Foreground`], so it is
/// above every panel and the canvas and takes the press there; and only the
/// zone's own rectangle — **never one area over the whole window**, which would
/// take every press in the app. A press starts the system's resize loop
/// ([`egui::ViewportCommand::BeginResize`]), which needs the button still down,
/// so it is sent on the press and not on a drag that egui recognises later.
///
/// ⚠️ **None while the window is maximized.** There is nothing to resize from
/// then — the system ignores the request — and a zone left standing would take
/// the top five points of the caption buttons and the close button's corner,
/// the one place in the window a thrown pointer is meant to land.
pub(crate) fn resize_zones(ctx: &egui::Context, chrome: Chrome) {
    if !chrome.draws_own_resize_border() || maximized(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    for (i, (direction, rect)) in zones(screen).into_iter().enumerate() {
        egui::Area::new(zone_id(i, direction))
            .order(Order::Foreground)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(rect.size(), POINTER_ONLY);
                zone_acts(ui, resp, direction);
            });
    }
}

/// One resize zone's behaviour, on the response of whatever senses it: the
/// direction's cursor on hover, and the system's resize loop on the press —
/// sent on the press, because `BeginResize` needs the button still down.
fn zone_acts(ui: &egui::Ui, resp: egui::Response, direction: ResizeDirection) {
    let resp = resp.on_hover_cursor(cursor(direction));
    if resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

#[cfg(test)]
mod tests {
    //! Plain backticks in this module's prose: `cargo doc` cannot see a
    //! `cfg(test)` module, so a link here is checked by nothing (§15 D319).
    use super::*;

    /// **Each host asks for its own window**: macOS keeps its decorations and
    /// traffic lights under a transparent title bar; Windows and Linux open
    /// undecorated. Every host carries the app id.
    #[test]
    fn each_host_opens_the_window_it_draws_around() {
        let mac = Chrome::of(Host::MacOs).viewport(egui::ViewportBuilder::default());
        assert_eq!(mac.decorations, None, "macOS keeps its decorations");
        assert_eq!(mac.fullsize_content_view, Some(true));
        assert_eq!(mac.titlebar_shown, Some(false));
        assert_eq!(mac.app_id.as_deref(), Some(APP_ID));
        for host in [Host::Windows, Host::Linux] {
            let b = Chrome::of(host).viewport(egui::ViewportBuilder::default());
            assert_eq!(b.decorations, Some(false), "{host:?} draws its own");
            assert_eq!(b.app_id.as_deref(), Some(APP_ID));
        }
    }

    /// **The capabilities agree with each other**: a host that draws its own
    /// buttons draws its own resize border and needs no leading inset; macOS
    /// the reverse.
    #[test]
    fn the_capabilities_agree_per_host() {
        for host in [Host::Windows, Host::Linux] {
            let c = Chrome::of(host);
            assert!(c.draws_own_controls() && c.draws_own_resize_border());
            assert_eq!(c.leading_inset(), 0.0);
        }
        let mac = Chrome::of(Host::MacOs);
        assert!(!mac.draws_own_controls() && !mac.draws_own_resize_border());
        assert!(mac.leading_inset() > 50.0, "clear of the traffic lights");
    }

    /// **The zones cover the frame and contest no point**: every edge is
    /// `EDGE` deep, each corner an L of two `EDGE`-deep arms `CORNER` long, and
    /// no two zones overlap — so a press has one answer — while every point of
    /// the window's border belongs to one of them.
    ///
    /// **And the L is an L** (§15 D973, `[X2-L3-01]`): the inside of the
    /// corner, 8 points in from both edges, is nobody's — on the top right that
    /// is the close button. The corners were `Rect::union`s, which are bounding
    /// boxes, and this test sampled only the border, so a square passed it.
    /// **Flip-check, run**: each corner back to the square
    /// `rect(r - CORNER, t, r, t + CORNER)` and its vertical arm dropped fails
    /// on *"the inside of the top-right corner"*.
    #[test]
    fn the_resize_zones_tile_the_border() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let z = zones(screen);
        for (i, (_, a)) in z.iter().enumerate() {
            for (_, b) in &z[i + 1..] {
                assert!(
                    !a.intersects(*b) || a.intersect(*b).area() == 0.0,
                    "{a:?} and {b:?} overlap"
                );
            }
        }
        for x in [0.5, 7.0, 400.0, 793.0, 799.5] {
            for y in [0.5, 599.5] {
                let p = Pos2::new(x, y);
                assert!(
                    z.iter().any(|(_, r)| r.contains(p)),
                    "the border point {p:?} has a zone"
                );
            }
        }
        for y in [0.5, 7.0, 300.0, 593.0, 599.5] {
            for x in [0.5, 799.5] {
                let p = Pos2::new(x, y);
                assert!(
                    z.iter().any(|(_, r)| r.contains(p)),
                    "the border point {p:?} has a zone"
                );
            }
        }
        assert!(
            !z.iter().any(|(_, r)| r.contains(Pos2::new(400.0, 300.0))),
            "and the middle has none"
        );
        let at = |p: Pos2| z.iter().find(|(_, r)| r.contains(p)).map(|(d, _)| *d);
        assert_eq!(
            at(Pos2::new(792.0, 8.0)),
            None,
            "the inside of the top-right corner"
        );
        assert_eq!(at(Pos2::new(797.0, 12.0)), Some(ResizeDirection::NorthEast));
        assert_eq!(at(Pos2::new(788.0, 2.0)), Some(ResizeDirection::NorthEast));
        assert_eq!(at(Pos2::new(8.0, 592.0)), None, "nor of the bottom-left");
    }

    const SCREEN: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 600.0));
    const BAR: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(800.0, 46.0));

    /// One frame of a bar laid as the app lays one — the drag strip first, the
    /// caption buttons at the right end of a `right_to_left` row with the app's
    /// own 5-point spacing — and the resize zones, with `events` delivered and
    /// the window reported `maximized` or not. Returns the viewport commands
    /// the frame sent.
    fn frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        maximized: bool,
    ) -> Vec<egui::ViewportCommand> {
        frame_with(ctx, events, maximized, true, None)
    }

    /// `frame` with the bar's `act`, and with `card` — `Some(lifted)` puts a
    /// modal card up after the bar, through `settings::card` as every card in
    /// the app goes, and then calls `above_modal` when `lifted`.
    fn frame_with(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        maximized: bool,
        act: bool,
        card: Option<bool>,
    ) -> Vec<egui::ViewportCommand> {
        let mut input = egui::RawInput {
            screen_rect: Some(SCREEN),
            time: Some(ctx.input(|i| i.time) + 0.1),
            events,
            ..Default::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                maximized: Some(maximized),
                ..Default::default()
            },
        );
        let chrome = Chrome::of(Host::Windows);
        let out = ctx.run_ui(input, |ui| {
            resize_zones(ui.ctx(), chrome);
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(BAR)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
                |ui| {
                    drag_strip(ui, BAR, act);
                    ui.spacing_mut().item_spacing.x = 5.0;
                    caption_buttons(ui, chrome, act);
                    after_buttons(ui, 14.0, 5.0);
                    let _ = ui.button("Settings");
                },
            );
            if let Some(lifted) = card {
                let _ = crate::settings::card("probe-card", ui.ctx(), |ui| {
                    ui.set_width(200.0);
                    ui.label("A card");
                });
                if lifted {
                    above_modal(ui.ctx(), chrome);
                }
            }
        });
        out.viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default()
    }

    fn press(at: Pos2, down: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: down,
            modifiers: Default::default(),
        }
    }

    /// A click at `at`, over the frames egui needs to see one; every command
    /// those frames sent.
    fn click(ctx: &egui::Context, at: Pos2, maximized: bool) -> Vec<egui::ViewportCommand> {
        let mut sent = frame(ctx, vec![egui::Event::PointerMoved(at)], maximized);
        sent.extend(frame(ctx, vec![press(at, true)], maximized));
        sent.extend(frame(ctx, vec![press(at, false)], maximized));
        sent.extend(frame(ctx, Vec::new(), maximized));
        sent
    }

    fn fresh() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = frame(&ctx, Vec::new(), false);
        ctx
    }

    /// **The three buttons sit flush right, touching, and each sends its own
    /// command**: close at 754–800 sends `Close` — the system's request, which
    /// the app's close interception answers — maximize at 708–754 sends
    /// `Maximized(true)`, or `Maximized(false)` once the window reports itself
    /// maximized, and minimize at 662–708 sends `Minimized(true)`.
    ///
    /// **Flip run**, the buttons left in the bar's 5-point spacing (the scope
    /// in `caption_buttons` removed): fails on *"minimize"*, the press at 705
    /// sending `Maximized(true)`. A first draft pressed at 663, inside minimize
    /// either way, and that flip passed — the press has to sit where the two
    /// layouts disagree.
    #[test]
    fn each_caption_button_sends_its_command() {
        use egui::ViewportCommand as C;
        let ctx = fresh();
        let has = |sent: &[C], c: &C| sent.iter().any(|s| s == c);
        assert!(
            has(&click(&ctx, Pos2::new(790.0, 23.0), false), &C::Close),
            "close"
        );
        let ctx = fresh();
        assert!(
            has(
                &click(&ctx, Pos2::new(731.0, 23.0), false),
                &C::Maximized(true)
            ),
            "maximize"
        );
        let ctx = fresh();
        assert!(
            has(
                &click(&ctx, Pos2::new(731.0, 23.0), true),
                &C::Maximized(false)
            ),
            "restore, while maximized"
        );
        let ctx = fresh();
        // 705: inside minimize when the three touch (662–708), inside maximize
        // when the bar's spacing parts them (703–749).
        let sent = click(&ctx, Pos2::new(705.0, 23.0), false);
        assert!(has(&sent, &C::Minimized(true)), "minimize: {sent:?}");
        assert!(
            !sent.iter().any(|c| matches!(c, C::StartDrag)),
            "and a button press is no drag"
        );
    }

    /// **The empty bar drags the window and a double-click on it maximizes**,
    /// while a press on a widget in the bar does neither.
    ///
    /// **Flip run**, the strip's arming ignored (a drag starts the move from any
    /// press): fails on *"a press-drag on a button is the button's"* with
    /// `[StartDrag]` — the predicted site, and the defect the first draft had:
    /// egui gives a click-only button the click and the strip beneath it the
    /// drag.
    #[test]
    fn the_empty_bar_drags_and_double_clicks_maximize() {
        use egui::ViewportCommand as C;
        let ctx = fresh();
        let at = Pos2::new(300.0, 23.0);
        let mut sent = frame(&ctx, vec![egui::Event::PointerMoved(at)], false);
        sent.extend(frame(&ctx, vec![press(at, true)], false));
        for dx in [4.0, 12.0, 30.0] {
            sent.extend(frame(
                &ctx,
                vec![egui::Event::PointerMoved(at + Vec2::new(dx, 0.0))],
                false,
            ));
        }
        assert!(
            sent.iter().any(|c| matches!(c, C::StartDrag)),
            "drags: {sent:?}"
        );

        let ctx = fresh();
        let mut sent = Vec::new();
        for _ in 0..2 {
            sent.extend(frame(&ctx, vec![press(at, true)], false));
            sent.extend(frame(&ctx, vec![press(at, false)], false));
        }
        assert!(sent.contains(&C::Maximized(true)), "double-click: {sent:?}");

        // A press-drag that starts on a caption button.
        let ctx = fresh();
        let on = Pos2::new(731.0, 23.0);
        let mut sent = frame(&ctx, vec![egui::Event::PointerMoved(on)], false);
        sent.extend(frame(&ctx, vec![press(on, true)], false));
        for dx in [4.0, 12.0, 30.0] {
            sent.extend(frame(
                &ctx,
                vec![egui::Event::PointerMoved(on - Vec2::new(dx, 0.0))],
                false,
            ));
        }
        assert!(
            !sent.iter().any(|c| matches!(c, C::StartDrag)),
            "a press-drag on a button is the button's: {sent:?}"
        );
    }

    /// **A press on the window's edge starts the system's resize, in the edge's
    /// direction — and only while the window is not maximized**, when the zone
    /// would otherwise sit over the close button's corner.
    ///
    /// **Flip run**, the zones kept while maximized: fails on *"none while
    /// maximized"*, the corner's press a `BeginResize(NorthEast)` — the
    /// predicted site.
    #[test]
    fn a_press_on_the_edge_resizes_unless_maximized() {
        use egui::ViewportCommand as C;
        let ctx = fresh();
        let sent = click(&ctx, Pos2::new(1.0, 300.0), false);
        assert!(
            sent.contains(&C::BeginResize(ResizeDirection::West)),
            "the left edge: {sent:?}"
        );
        let ctx = fresh();
        let sent = click(&ctx, Pos2::new(797.0, 597.0), false);
        assert!(
            sent.contains(&C::BeginResize(ResizeDirection::SouthEast)),
            "the corner"
        );

        let ctx = fresh();
        let sent = click(&ctx, Pos2::new(798.0, 1.0), true);
        assert!(
            !sent.iter().any(|c| matches!(c, C::BeginResize(_))),
            "none while maximized: {sent:?}"
        );
        assert!(sent.contains(&C::Close), "and the corner is close's again");
    }

    /// One frame of present mode as the app draws it: a canvas over the whole
    /// window, the resize zones and the strip, with `events` delivered. Returns
    /// the viewport commands the frame sent and whether the canvas was clicked.
    fn present_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        maximized: bool,
    ) -> (Vec<egui::ViewportCommand>, bool) {
        let mut input = egui::RawInput {
            screen_rect: Some(SCREEN),
            time: Some(ctx.input(|i| i.time) + 0.1),
            events,
            ..Default::default()
        };
        input.viewports.insert(
            egui::ViewportId::ROOT,
            egui::ViewportInfo {
                maximized: Some(maximized),
                ..Default::default()
            },
        );
        let chrome = Chrome::of(Host::Windows);
        let mut canvas = false;
        let out = ctx.run_ui(input, |ui| {
            canvas = ui
                .interact(SCREEN, Id::new("canvas"), Sense::click_and_drag())
                .clicked();
            resize_zones(ui.ctx(), chrome);
            present_strip(ui.ctx(), chrome);
        });
        let sent = out
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| v.commands.clone())
            .unwrap_or_default();
        (sent, canvas)
    }

    /// The pointer moved through `path`, then a click at its last point;
    /// every command sent, and whether the canvas took a click.
    fn present_click(
        ctx: &egui::Context,
        path: &[Pos2],
        maximized: bool,
    ) -> (Vec<egui::ViewportCommand>, bool) {
        let (mut sent, mut canvas) = (Vec::new(), false);
        let at = *path.last().unwrap();
        let events = path
            .iter()
            .map(|p| vec![egui::Event::PointerMoved(*p)])
            .chain([vec![press(at, true)], vec![press(at, false)], Vec::new()]);
        for e in events {
            let (s, c) = present_frame(ctx, e, maximized);
            sent.extend(s);
            canvas |= c;
        }
        (sent, canvas)
    }

    fn fresh_present() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = present_frame(&ctx, Vec::new(), false);
        ctx
    }

    /// **Present mode's strip comes out at the top edge, and its close button
    /// closes** — and only from the edge: a pointer that arrives at the
    /// strip's height from the canvas below finds the canvas there, and one
    /// that has left the strip has hidden it.
    ///
    /// **Flip run**, revealed from anywhere on the strip (`strip.contains(p)`
    /// for the edge band in the unrevealed arm): fails on *"the canvas keeps
    /// its click there"*. Predicted on *"not from below"* and wrong — the strip
    /// came out under the pointer and took the press from the canvas, but no
    /// `Close` arrived in the frames this test pumps. So it is the canvas
    /// assertion that has the teeth for this case, not the command one.
    #[test]
    fn the_present_strip_comes_out_at_the_top_edge_and_closes() {
        use egui::ViewportCommand as C;
        let close = Pos2::new(790.0, 16.0);
        let middle = Pos2::new(400.0, 300.0);

        let ctx = fresh_present();
        let (sent, canvas) = present_click(&ctx, &[middle, close], false);
        assert!(!sent.contains(&C::Close), "not from below: {sent:?}");
        assert!(canvas, "the canvas keeps its click there");

        let ctx = fresh_present();
        let (sent, canvas) = present_click(&ctx, &[Pos2::new(790.0, 2.0), close], false);
        assert!(sent.contains(&C::Close), "from the edge: {sent:?}");
        assert!(!canvas, "and the canvas under it saw nothing");

        let ctx = fresh_present();
        let (sent, _) = present_click(&ctx, &[Pos2::new(790.0, 2.0), middle, close], false);
        assert!(!sent.contains(&C::Close), "left, and hidden: {sent:?}");
    }

    /// **A drag carried to the top of the window brings nothing out**: the
    /// strip is for a pointer at rest, and a shape dragged up to the edge must
    /// not have a strip opened over it.
    ///
    /// **Flip run**, the `!down` dropped from the reveal: fails at the
    /// assertion, the strip out under the held button — the predicted site.
    #[test]
    fn a_held_button_does_not_bring_the_strip_out() {
        let ctx = fresh_present();
        let start = Pos2::new(400.0, 300.0);
        let _ = present_frame(&ctx, vec![egui::Event::PointerMoved(start)], false);
        let _ = present_frame(&ctx, vec![press(start, true)], false);
        for y in [200.0, 60.0, 2.0] {
            let _ = present_frame(
                &ctx,
                vec![egui::Event::PointerMoved(Pos2::new(400.0, y))],
                false,
            );
        }
        let out = ctx.data(|d| d.get_temp::<bool>(Id::new("present-strip")));
        assert_eq!(out, Some(false), "a held button reveals nothing");
    }

    /// **The edge that reveals the strip still resizes the window**, because
    /// the strip is on a layer beneath the resize zones — and a maximized
    /// window, which has no zones, gives the edge to the strip.
    ///
    /// **Flip run**, the strip on `Order::Foreground` beside the zones: fails
    /// on *"the top edge resizes"*, the strip newer on that layer and so on top
    /// of the zone — the predicted site.
    #[test]
    fn the_top_edge_still_resizes_under_the_strip() {
        use egui::ViewportCommand as C;
        let edge = Pos2::new(400.0, 1.0);
        let ctx = fresh_present();
        let (sent, _) = present_click(&ctx, &[edge], false);
        assert!(
            sent.contains(&C::BeginResize(ResizeDirection::North)),
            "the top edge resizes: {sent:?}"
        );

        let ctx = fresh_present();
        let (sent, canvas) = present_click(&ctx, &[edge, Pos2::new(731.0, 16.0)], true);
        assert!(
            sent.contains(&C::Maximized(false)),
            "maximized, the strip restores: {sent:?}"
        );
        assert!(!canvas);
    }

    /// A click at `at` over the frames egui needs, with the bar's `act` and a
    /// `card` as `frame_with` takes them.
    fn click_with(
        ctx: &egui::Context,
        at: Pos2,
        act: bool,
        card: Option<bool>,
    ) -> Vec<egui::ViewportCommand> {
        let mut sent = frame_with(ctx, vec![egui::Event::PointerMoved(at)], false, act, card);
        sent.extend(frame_with(ctx, vec![press(at, true)], false, act, card));
        sent.extend(frame_with(ctx, vec![press(at, false)], false, act, card));
        sent.extend(frame_with(ctx, Vec::new(), false, act, card));
        sent
    }

    /// **A modal card leaves the window's controls working** (§15 D973,
    /// `[X2-L1-02]`): close, maximize and the west edge each send their command
    /// with a card up, where `egui::Modal`'s refusal of every layer beneath it
    /// had left all three dead.
    ///
    /// **Flip-check, run**: the card without `above_modal` (`Some(false)`) is
    /// the control, and it sends nothing — asserted below, so a harness that
    /// stopped putting a card up would fail here rather than pass vacuously.
    /// And without the zones laid again inside the lifted area (the `zone`
    /// widgets in `above_modal`), it fails on the top edge, `[400.0 1.0]`
    /// sending nothing — the strip takes the press. That is also how the
    /// first draft failed, which raised the zone areas after the lifted one
    /// and could not order them by doing so.
    #[test]
    fn a_modal_card_leaves_the_windows_controls_working() {
        use egui::ViewportCommand as C;
        // The top edge, a top corner and a side within the bar's height are
        // where the lifted strip and buttons overlap the zones, and the zones
        // must win there as they do with no card.
        let cases = [
            (Pos2::new(790.0, 23.0), C::Close),
            (Pos2::new(731.0, 23.0), C::Maximized(true)),
            (Pos2::new(1.0, 300.0), C::BeginResize(ResizeDirection::West)),
            (
                Pos2::new(400.0, 1.0),
                C::BeginResize(ResizeDirection::North),
            ),
            (
                Pos2::new(799.0, 1.0),
                C::BeginResize(ResizeDirection::NorthEast),
            ),
            (Pos2::new(1.0, 30.0), C::BeginResize(ResizeDirection::West)),
        ];
        for (at, want) in cases {
            let ctx = fresh();
            let _ = frame_with(&ctx, Vec::new(), false, true, Some(false));
            let blocked = click_with(&ctx, at, true, Some(false));
            assert!(
                !blocked.contains(&want),
                "the control: {at:?} sent {blocked:?}"
            );

            let ctx = fresh();
            let _ = frame_with(&ctx, Vec::new(), false, true, Some(true));
            let sent = click_with(&ctx, at, true, Some(true));
            assert!(sent.contains(&want), "with a card up, {at:?} sent {sent:?}");
        }
    }

    /// **A click that dismisses a menu is spent doing so** (§15 D558, D973,
    /// `[R1-L2-02]`): with `act` false the caption buttons send nothing, and a
    /// double-click whose first press was spent maximizes nothing.
    ///
    /// **Flip-check, run**: `three_buttons` ignoring `act` fails on *"close"*;
    /// `strip` ignoring the earlier spent press fails on *"a dismissal and a
    /// click"* with `[Maximized(true)]`.
    #[test]
    fn a_spent_click_moves_no_window() {
        use egui::ViewportCommand as C;
        let ctx = fresh();
        let sent = click_with(&ctx, Pos2::new(790.0, 23.0), false, None);
        assert!(!sent.contains(&C::Close), "close: {sent:?}");
        let sent = click_with(&ctx, Pos2::new(731.0, 23.0), false, None);
        assert!(!sent.contains(&C::Maximized(true)), "maximize: {sent:?}");

        // A press that dismissed a menu, then one that did not, quickly enough
        // to be a double-click on the empty bar.
        let ctx = fresh();
        let at = Pos2::new(300.0, 23.0);
        let mut sent = frame_with(
            &ctx,
            vec![egui::Event::PointerMoved(at)],
            false,
            false,
            None,
        );
        sent.extend(frame_with(&ctx, vec![press(at, true)], false, false, None));
        sent.extend(frame_with(&ctx, vec![press(at, false)], false, true, None));
        sent.extend(frame_with(&ctx, vec![press(at, true)], false, true, None));
        sent.extend(frame_with(&ctx, vec![press(at, false)], false, true, None));
        assert!(
            !sent.contains(&C::Maximized(true)),
            "a dismissal and a click: {sent:?}"
        );
    }
}
