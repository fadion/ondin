//! The **Container** and **Item** cards — container layout's inspector
//! (`architecture.md` §5.3c, §15 D878), built from the maintainer's mockup
//! (`design/Layout Cards.dc.html`, and `LayoutGlyph.dc.html` for the pictures).
//!
//! **Container** is a frame's or a group's layout — `display`, and under `flex` its
//! direction, wrap, three alignments, two gaps and its padding; under `grid` its
//! flow, two track lists, four alignments, gaps and padding (the child module
//! `grid`, §15 D920). **Item** is a layer's place in its parent's layout —
//! `flex-grow`, `flex-shrink`, `flex-basis` and `align-self` in a flex container,
//! its lines and both self-alignments in a grid, and the four size limits in
//! either — or, for a layer out of the flow, why it is out and the one button that
//! brings it back. Every label is a CSS property's name
//! and every value a CSS value (§15 D867), **written in sentence case** —
//! `justify-content` reads *Justify content*, `flex-start` *Flex start* (§15 D884).
//!
//! **Cards run from the layer in its parent to its children**: Transform, Position,
//! Item, then Container and the Layout grid, then paint. A frame that is both an item
//! and a container gets both cards, Item first.
//!
//! **Neither card has a header badge** — the mockup's *flex · row*, *out of flow*,
//! *in flow* — and that is ruled, not an omission (§15 D897): a badge is for a
//! state a card has nowhere else to say, which is why the Position card has
//! *Absolute* (`OndinApp::panel_badged`). The Item card says out-of-flow in a whole
//! block, and Container's badge would repeat the display row under it.
//!
//! Three things the cards say that are rulings rather than readings:
//!
//! - **A hidden layer is out of the flow** (§15 D881) — the Item card says so in
//!   words, which is what made the session's default a ruling.
//! - **A resize's flips come with a receipt** (§15 D880): the growth a resize stops
//!   is named, with an Undo that is the whole step (`session::FlexReceipt`).
//! - **Sizing modes are offered per kind** (§15 D879): the W and H fields' unit is a
//!   menu of only the modes that mean something different for that layer
//!   ([`size_modes`]), and typing or dragging a number writes px.
//!
//! The fourth is a consequence, not a ruling: **an in-flow item's X and Y are
//! inert** (§15 D882), because its container places it and a typed position would
//! be dropped at the commit door (`build::keep_flex_sizes`, §15 D877's amendment).

use super::inspector::baked_ops;
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui::{self, Prefix, Scrub, Suffix};
use eframe::egui;
use ondin_core::container::{
    self, AlignContent, AlignItems, Dimension, Display, Flex, FlexDirection, FlexWrap, Grid,
    JustifyContent, LayoutItem,
};
use ondin_core::kurbo::Size;
use ondin_core::{NodeId, NodeKind, Operation, Transaction};

/// Grid's rows of both cards — the track lists, `grid-auto-flow`, the grid
/// alignments, and an item's lines (§15 D920). A child module so it shares this
/// file's private pieces: `glyph_combo`, `shared`, `edited`, the gap and padding
/// rows.
mod grid;

// --- the pictures ---------------------------------------------------------

/// One of the mockup's layout pictures (`LayoutGlyph.dc.html`), drawn on its
/// 16-unit grid rather than taken from the icon font, which has nothing for a
/// justification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Glyph {
    /// `display: none` — three squares scattered, children keeping their own places.
    /// **Never an eye**: the mockup's note is that `none` must not read as hidden.
    DisplayNone,
    DisplayFlex,
    DisplayGrid,
    Justify(JustifyContent),
    Align(AlignItems),
    Content(AlignContent),
}

/// One stroke of a [`Glyph`], in grid units: a filled bar, or an edge line (the
/// container's side the items are pushed against). `a` is the opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mark {
    Bar {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        a: f32,
    },
    Edge {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        a: f32,
        dashed: bool,
    },
}

impl Mark {
    /// Mirrored across the grid's diagonal — a row's picture turned into a column's.
    fn transposed(self) -> Mark {
        match self {
            Mark::Bar { x, y, w, h, a } => Mark::Bar {
                x: y,
                y: x,
                w: h,
                h: w,
                a,
            },
            Mark::Edge {
                x1,
                y1,
                x2,
                y2,
                a,
                dashed,
            } => Mark::Edge {
                x1: y1,
                y1: x1,
                x2: y2,
                y2: x2,
                a,
                dashed,
            },
        }
    }

    fn flipped(self, fx: bool, fy: bool) -> Mark {
        let f = |v: f32, on: bool| if on { 16.0 - v } else { v };
        match self {
            Mark::Bar { x, y, w, h, a } => Mark::Bar {
                x: if fx { 16.0 - x - w } else { x },
                y: if fy { 16.0 - y - h } else { y },
                w,
                h,
                a,
            },
            Mark::Edge {
                x1,
                y1,
                x2,
                y2,
                a,
                dashed,
            } => Mark::Edge {
                x1: f(x1, fx),
                y1: f(y1, fy),
                x2: f(x2, fx),
                y2: f(y2, fy),
                a,
                dashed,
            },
        }
    }
}

const fn bar(x: f32, y: f32, w: f32, h: f32) -> Mark {
    Mark::Bar { x, y, w, h, a: 1.0 }
}

const fn edge(x1: f32, y1: f32, x2: f32, y2: f32, a: f32) -> Mark {
    Mark::Edge {
        x1,
        y1,
        x2,
        y2,
        a,
        dashed: false,
    }
}

/// The container's four sides, as the mockup draws them.
const fn left(a: f32) -> Mark {
    edge(1.5, 2.0, 1.5, 14.0, a)
}
const fn right(a: f32) -> Mark {
    edge(14.5, 2.0, 14.5, 14.0, a)
}
const fn top(a: f32) -> Mark {
    edge(2.0, 1.5, 14.0, 1.5, a)
}
const fn bottom(a: f32) -> Mark {
    edge(2.0, 14.5, 14.0, 14.5, a)
}

/// A justification drawn along the x axis — a row's main axis.
fn justify_marks(j: JustifyContent) -> Vec<Mark> {
    use JustifyContent as J;
    match j {
        J::Start => vec![left(1.0), bar(3.5, 5.0, 3.0, 6.0), bar(8.0, 5.0, 3.0, 6.0)],
        J::End => vec![right(1.0), bar(5.0, 5.0, 3.0, 6.0), bar(9.5, 5.0, 3.0, 6.0)],
        J::Center => vec![
            left(0.3),
            right(0.3),
            bar(4.25, 5.0, 3.0, 6.0),
            bar(8.75, 5.0, 3.0, 6.0),
        ],
        J::SpaceBetween => vec![
            left(0.3),
            right(0.3),
            bar(2.5, 5.0, 2.0, 6.0),
            bar(7.0, 5.0, 2.0, 6.0),
            bar(11.5, 5.0, 2.0, 6.0),
        ],
        J::SpaceAround => vec![
            left(0.3),
            right(0.3),
            bar(3.25, 5.0, 2.0, 6.0),
            bar(7.0, 5.0, 2.0, 6.0),
            bar(10.75, 5.0, 2.0, 6.0),
        ],
        J::SpaceEvenly => vec![
            left(0.3),
            right(0.3),
            bar(4.0, 5.0, 2.0, 6.0),
            bar(7.0, 5.0, 2.0, 6.0),
            bar(10.0, 5.0, 2.0, 6.0),
        ],
    }
}

/// Every mark of `g`, drawn for a `row` that does not wrap in reverse — the
/// orientation the mockup draws them in. [`Orient`] turns them from there.
fn marks(g: Glyph) -> Vec<Mark> {
    use AlignItems as A;
    match g {
        Glyph::DisplayNone => vec![
            bar(2.0, 2.5, 4.5, 4.5),
            bar(9.5, 4.5, 4.5, 4.5),
            bar(4.5, 10.0, 4.0, 4.0),
        ],
        Glyph::DisplayFlex => vec![
            bar(2.0, 4.0, 3.3, 8.0),
            bar(6.35, 4.0, 3.3, 8.0),
            bar(10.7, 4.0, 3.3, 8.0),
        ],
        Glyph::DisplayGrid => vec![
            bar(2.5, 2.5, 4.5, 4.5),
            bar(9.0, 2.5, 4.5, 4.5),
            bar(2.5, 9.0, 4.5, 4.5),
            bar(9.0, 9.0, 4.5, 4.5),
        ],
        Glyph::Justify(j) => justify_marks(j),
        Glyph::Align(a) => match a {
            A::Stretch => vec![
                top(1.0),
                bottom(1.0),
                bar(4.0, 3.0, 3.0, 10.0),
                bar(9.0, 3.0, 3.0, 10.0),
            ],
            A::Start => vec![top(1.0), bar(4.0, 3.0, 3.0, 5.0), bar(9.0, 3.0, 3.0, 8.0)],
            A::End => vec![
                bottom(1.0),
                bar(4.0, 8.0, 3.0, 5.0),
                bar(9.0, 5.0, 3.0, 8.0),
            ],
            A::Center => vec![
                top(0.3),
                bottom(0.3),
                bar(4.0, 5.5, 3.0, 5.0),
                bar(9.0, 4.0, 3.0, 8.0),
            ],
            A::Baseline => vec![
                Mark::Edge {
                    x1: 1.5,
                    y1: 8.5,
                    x2: 14.5,
                    y2: 8.5,
                    a: 0.8,
                    dashed: true,
                },
                bar(4.0, 5.0, 3.0, 6.0),
                bar(9.0, 3.0, 3.0, 9.0),
            ],
        },
        // **`align-content` reuses the justifications on the cross axis** — the
        // mockup's own rule — so a row's lines are the justify pictures turned
        // through the diagonal. `stretch` is the one drawn for itself.
        Glyph::Content(AlignContent::Stretch) => vec![
            top(1.0),
            bottom(1.0),
            bar(3.0, 3.0, 10.0, 4.25),
            bar(3.0, 8.75, 10.0, 4.25),
        ],
        Glyph::Content(c) => justify_marks(content_as_justify(c))
            .into_iter()
            .map(Mark::transposed)
            .collect(),
    }
}

/// The justification an `align-content` value draws as. `Stretch` has its own
/// picture and never asks.
fn content_as_justify(c: AlignContent) -> JustifyContent {
    match c {
        AlignContent::Start | AlignContent::Stretch => JustifyContent::Start,
        AlignContent::End => JustifyContent::End,
        AlignContent::Center => JustifyContent::Center,
        AlignContent::SpaceBetween => JustifyContent::SpaceBetween,
        AlignContent::SpaceAround => JustifyContent::SpaceAround,
        AlignContent::SpaceEvenly => JustifyContent::SpaceEvenly,
    }
}

/// How a [`Glyph`] is turned to match its container: through the diagonal for a
/// column, and mirrored along whichever axis runs backwards.
///
/// ⚠️ **Not a rotation, which is what the mockup's glyph file does** (its `rot`
/// prop). A rotation that takes a row's justify-start — its wall on the left — to a
/// column's wall on the top takes align-start's wall from the top to the *right*,
/// where a column's cross start is on the left. The diagonal takes both where they
/// belong, so it is one rule for every picture instead of an angle per kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Orient {
    transpose: bool,
    flip_x: bool,
    flip_y: bool,
}

impl Orient {
    /// `g` as `flex` lays it out: a column transposes everything, a reversed
    /// direction mirrors the justifications along the main axis, and `wrap-reverse`
    /// mirrors the cross axis — where CSS swaps cross-start and cross-end, so
    /// `align-items: flex-start` really does sit at the bottom of a row.
    pub(crate) fn of(g: Glyph, flex: &Flex) -> Orient {
        let row = flex.direction.is_row();
        let (main_flip, cross_flip) = match g {
            Glyph::Justify(_) => (
                matches!(
                    flex.direction,
                    FlexDirection::RowReverse | FlexDirection::ColumnReverse
                ),
                false,
            ),
            Glyph::Align(_) | Glyph::Content(_) => (false, flex.wrap == FlexWrap::WrapReverse),
            _ => return Orient::default(),
        };
        let (flip_x, flip_y) = if row {
            (main_flip, cross_flip)
        } else {
            (cross_flip, main_flip)
        };
        Orient {
            transpose: !row,
            flip_x,
            flip_y,
        }
    }

    fn apply(self, m: Mark) -> Mark {
        let m = if self.transpose { m.transposed() } else { m };
        m.flipped(self.flip_x, self.flip_y)
    }
}

/// Paint `g` centred on `center`, `size` points square, in `ink`.
pub(crate) fn paint_glyph(
    p: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    g: Glyph,
    orient: Orient,
    ink: egui::Color32,
) {
    let s = size / 16.0;
    let o = center - egui::vec2(size, size) / 2.0;
    let at = |x: f32, y: f32| o + egui::vec2(x * s, y * s);
    for m in marks(g) {
        match orient.apply(m) {
            Mark::Bar { x, y, w, h, a } => {
                p.rect_filled(
                    egui::Rect::from_min_size(at(x, y), egui::vec2(w * s, h * s)),
                    egui::CornerRadius::same(1),
                    ink.gamma_multiply(a),
                );
            }
            Mark::Edge {
                x1,
                y1,
                x2,
                y2,
                a,
                dashed,
            } => {
                let stroke = egui::Stroke::new(1.2 * s, ink.gamma_multiply(a));
                let (from, to) = (at(x1, y1), at(x2, y2));
                if dashed {
                    let length = from.distance(to);
                    let dir = (to - from) / length.max(1e-3);
                    let (on, off) = (1.4 * s, 1.6 * s);
                    let mut t = 0.0;
                    while t < length {
                        let end = (t + on).min(length);
                        p.line_segment([from + dir * t, from + dir * end], stroke);
                        t += on + off;
                    }
                } else {
                    p.line_segment([from, to], stroke);
                }
            }
        }
    }
}

/// The size a glyph is drawn at inside a field or a menu row, and the room left
/// for it in the text beside it.
const GLYPH_PT: f32 = 14.0;
const GLYPH_GAP: f32 = 6.0;

// --- names ------------------------------------------------------------------
//
// **CSS's names, in sentence case** (§15 D884): `flex-start` reads *Flex start*,
// `justify-content` *Justify content* — the first letter raised and the hyphens
// gone, and nothing else changed, so each is still the CSS name a reader can look
// up. The model, the file and the snapshot keep CSS's own spelling; only the chrome
// is dressed. Inside a sentence (the receipt) they are lower-cased again.

pub(crate) fn justify_name(j: JustifyContent) -> &'static str {
    match j {
        JustifyContent::Start => "Flex start",
        JustifyContent::End => "Flex end",
        JustifyContent::Center => "Center",
        JustifyContent::SpaceBetween => "Space between",
        JustifyContent::SpaceAround => "Space around",
        JustifyContent::SpaceEvenly => "Space evenly",
    }
}

pub(crate) fn align_name(a: AlignItems) -> &'static str {
    match a {
        AlignItems::Stretch => "Stretch",
        AlignItems::Start => "Flex start",
        AlignItems::End => "Flex end",
        AlignItems::Center => "Center",
        AlignItems::Baseline => "Baseline",
    }
}

pub(crate) fn content_name(c: AlignContent) -> &'static str {
    match c {
        AlignContent::Stretch => "Stretch",
        AlignContent::Start => "Flex start",
        AlignContent::End => "Flex end",
        AlignContent::Center => "Center",
        AlignContent::SpaceBetween => "Space between",
        AlignContent::SpaceAround => "Space around",
        AlignContent::SpaceEvenly => "Space evenly",
    }
}

/// What each `flex-direction` cell does, for its tooltip (§15 D886): the name,
/// then the way the items run.
const DIRECTION_TIPS: [&str; 4] = [
    "Row — items run left to right",
    "Row reverse — items run right to left",
    "Column — items run top to bottom",
    "Column reverse — items run bottom to top",
];

/// What each `flex-wrap` cell does, for its tooltip (§15 D886).
const WRAP_TIPS: [&str; 3] = [
    "No wrap — every item stays on one line, shrinking to fit",
    "Wrap — items that do not fit start a new line",
    "Wrap reverse — new lines stack the other way",
];

/// What each `display` cell does, for its tooltip (§15 D886). `grid`'s said why
/// it could not be picked until the grid step built it (§15 D920).
const DISPLAY_TIPS: [&str; 3] = [
    "None — children keep their own places",
    "Flex — lay the children out in a row or a column",
    "Grid — lay the children out in rows and columns of tracks",
];

const JUSTIFY: [JustifyContent; 6] = [
    JustifyContent::Start,
    JustifyContent::End,
    JustifyContent::Center,
    JustifyContent::SpaceBetween,
    JustifyContent::SpaceAround,
    JustifyContent::SpaceEvenly,
];
const ALIGN: [AlignItems; 5] = [
    AlignItems::Stretch,
    AlignItems::Start,
    AlignItems::End,
    AlignItems::Center,
    AlignItems::Baseline,
];
const CONTENT: [AlignContent; 7] = [
    AlignContent::Stretch,
    AlignContent::Start,
    AlignContent::End,
    AlignContent::Center,
    AlignContent::SpaceBetween,
    AlignContent::SpaceAround,
    AlignContent::SpaceEvenly,
];
const DIRECTIONS: [FlexDirection; 4] = [
    FlexDirection::Row,
    FlexDirection::RowReverse,
    FlexDirection::Column,
    FlexDirection::ColumnReverse,
];
const WRAPS: [FlexWrap; 3] = [FlexWrap::NoWrap, FlexWrap::Wrap, FlexWrap::WrapReverse];

/// The value every one of `all` agrees on, or `None` for *mixed*.
fn shared<T: Copy + PartialEq, S>(all: &[S], f: impl Fn(&S) -> T) -> Option<T> {
    let first = f(all.first()?);
    all.iter().all(|s| f(s) == first).then_some(first)
}

/// A number field's digits replaced with **"Mixed"** while the selection
/// disagrees — showing one of the values would read as a claim that it is *the*
/// value.
///
/// §15 D130's rule, "Mixed" wherever five letters fit, and the mockup's screen
/// 08. It read a dash, copying `typography::mixed_text`, until §15 D892 closed
/// D878's *Fix* verdict on it — the maintainer's ruling that the layout cards
/// follow D130 before grid copies them. The unit beside a mixed sizing mode
/// reads a dash ([`size_field`]), and **that dash is not a mixed marker**: the
/// digits already say "Mixed", and the dash says *no single unit* — the same
/// meaning §15 D895 gives it beside a keyword, Webflow's unit slot. Room was never
/// the reason (`ui::Suffix` is laid out to its text and holds *fit content*); an
/// earlier line here said it was. The mixed segment rows use `ui::segment_mixed`,
/// one of D130's two dashes (§15 D892).
fn mixed_if(d: egui::DragValue<'_>, mixed: bool) -> egui::DragValue<'_> {
    if mixed {
        return d.custom_formatter(|_, _| ui::MIXED_WORD.into());
    }
    d
}

/// Whether the field behind `resp` has been edited since it was engaged — `moved`
/// is whether its number changed this frame — **held through the frame the
/// engagement ends**, which is the frame `OndinApp::edit_valve` commits on.
///
/// 🚨 **Without it every number field in these cards committed nothing** (§15
/// D885). They built their transaction only when the number moved *this frame*,
/// and read that number through the preview (`display_node`, which has to be read
/// or a scrub goes nowhere). On the release frame nothing moves, the field shows
/// the value its own preview installed, so the transaction handed to the valve on
/// the one frame it commits was empty — and the valve then dropped the preview.
/// Reported as a gap or a padding that follows the scrub and snaps back to zero.
/// The Position card never had it because a pinned inset's field builds its edit
/// unconditionally; these cannot, because a click in and out of a field must not
/// write the value it was showing (a mixed selection's first, or a keyword's
/// resolved size) to every subject.
///
/// One accessor for the read and the write — `data_mut` inside `data_mut`
/// deadlocks (`CLAUDE.md`).
fn edited(resp: &egui::Response, moved: bool) -> bool {
    let engaged = resp.dragged() || resp.has_focus();
    let id = resp.id.with("layout-field-edited");
    resp.ctx.data_mut(|d| {
        let now = d.get_temp::<bool>(id).unwrap_or(false) || moved;
        d.insert_temp(id, now && engaged);
        now
    })
}

// --- sizing modes -------------------------------------------------------------

/// How a W or H — or a basis, or a limit — is set: CSS's four ways of saying a
/// size (§15 D879).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SizeMode {
    Px,
    Percent,
    Auto,
    FitContent,
}

impl SizeMode {
    /// The mode as the field's unit shows it: lower case, as `px` and `%` are — a
    /// unit beside a number, not a label.
    pub(crate) fn word(self) -> &'static str {
        match self {
            SizeMode::Px => "px",
            SizeMode::Percent => "%",
            SizeMode::Auto => "auto",
            SizeMode::FitContent => "fit content",
        }
    }

    /// The mode as the unit's menu lists it — the units as they are, the two
    /// keywords in sentence case (§15 D884).
    fn label(self) -> &'static str {
        match self {
            SizeMode::Px => "px",
            SizeMode::Percent => "%",
            SizeMode::Auto => "Auto",
            SizeMode::FitContent => "Fit content",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            SizeMode::Px => "fixed size",
            SizeMode::Percent => "of the container",
            SizeMode::Auto => "size to content",
            SizeMode::FitContent => "size to content",
        }
    }
}

/// The modes a W or H field offers for a layer of `kind` — **only those that mean
/// something different for it** (§15 D879).
///
/// `container::style_of` tells only three of CSS's four apart, and which two it
/// merges depends on the kind: a **frame**'s `auto` is its stored size, which is
/// `px`, and `fit-content` hugs its children; a **group with a layout** has no stored
/// size, so `auto` and `fit-content` both hug; a **shape** is sized by its own
/// geometry either way. So a frame offers `px` and, with a layout of its own to hug,
/// `fit-content`; a group with a layout `px` and `auto`; a shape only `px`. `%`
/// joins any of them that is `laid_out` — in its parent's flow — and that layout can
/// resize (`container::can_stretch`, or the group's box); a percentage of nothing is
/// not a mode. **Fewer than two means no menu at all.**
pub(crate) fn size_modes(kind: &NodeKind, has_display: bool, laid_out: bool) -> Vec<SizeMode> {
    let mut modes = vec![SizeMode::Px];
    let resizable = match kind {
        NodeKind::Artboard { .. } => {
            if has_display {
                modes.push(SizeMode::FitContent);
            }
            true
        }
        NodeKind::Group if has_display => {
            modes.push(SizeMode::Auto);
            true
        }
        k => container::can_stretch(k),
    };
    if laid_out && resizable {
        modes.push(SizeMode::Percent);
    }
    modes
}

/// The mode a stored W or H `d` shows as, for a layer of `kind` — [`size_modes`]'
/// merges read back: a frame's or a shape's `auto` is its stored size, `px`; a
/// group's `fit-content` hugs exactly as its `auto` does.
pub(crate) fn size_mode_of(kind: &NodeKind, d: Dimension) -> SizeMode {
    match (kind, d) {
        (_, Dimension::Percent(_)) => SizeMode::Percent,
        (NodeKind::Group, Dimension::Px(_)) => SizeMode::Px,
        (NodeKind::Group, _) => SizeMode::Auto,
        (NodeKind::Artboard { .. }, Dimension::FitContent) => SizeMode::FitContent,
        _ => SizeMode::Px,
    }
}

/// A basis's or a limit's mode, where every stored value is its own mode.
fn plain_mode(d: Dimension) -> SizeMode {
    match d {
        Dimension::Px(_) => SizeMode::Px,
        Dimension::Percent(_) => SizeMode::Percent,
        Dimension::Auto => SizeMode::Auto,
        Dimension::FitContent => SizeMode::FitContent,
    }
}

/// What a [`size_field`] did this frame.
pub(crate) struct SizeEdit {
    /// The number's own response, for the valve.
    pub resp: egui::Response,
    /// A mode picked from the unit's menu — committed directly, a conversion
    /// rather than an edit, as a unit flip is (`ui::value_field_suffixed`).
    pub picked: Option<SizeMode>,
    /// The number, from the frame it was typed or dragged away from what it showed
    /// through the frame the field lets go ([`edited`], §15 D885) — so the valve's
    /// committing frame has it too.
    pub typed: Option<f64>,
}

/// A value field whose unit is a **menu of sizing modes** (§15 D879).
///
/// **What the digits show is the maintainer's rule, §15 D895: a number where the
/// number is a real size of the layer, the keyword where the property is unset.**
///
/// - `number` given: the size in the unit the mode names — px, a percentage, or
///   for a keyword the size it resolved to, the keyword standing where the unit
///   would be. The Transform card's W and H always pass one: a hugging frame
///   *is* 150 wide, and Figma's and Framer's W fields say so the same way.
/// - `number` absent and the mode a keyword: the keyword in the digits' place,
///   `under` beneath it so a scrub starts from there, and **no unit at all** — the
///   Position card's unpinned inset (§15 D890). The Item card's basis and limits:
///   an unset `min-width` has no number worth showing. It carried a dash for the
///   unit that opened the menu until the maintainer ruled it out (§15 D906): the
///   keyword already says how the size is set, and typing or dragging a number is
///   how to leave it, landing in px with the menu back.
/// - `number` absent and no mode: the selection disagrees, and the digits read
///   "Mixed" ([`mixed_if`], §15 D892).
///
/// Typing or dragging writes the number in px, or in % while the mode is %.
pub(crate) fn size_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    mode: Option<SizeMode>,
    number: Option<f64>,
    under: f64,
    modes: &[SizeMode],
) -> SizeEdit {
    let mut v = number.unwrap_or(under);
    let start = v;
    let percent = mode == Some(SizeMode::Percent);
    // The keyword standing in the digits, when there is no number to show.
    let word = match (number, mode) {
        (None, Some(m @ (SizeMode::Auto | SizeMode::FitContent))) => Some(m.word()),
        _ => None,
    };
    let (resp, unit) = ui::value_field_unit(
        ui,
        size,
        prefix,
        // No unit under a keyword (§15 D906).
        word.is_none().then_some(Suffix {
            text: match (number, mode) {
                (Some(_), Some(m)) => m.word(),
                _ => "–",
            },
            clickable: modes.len() > 1,
            tooltip: "How this size is set",
        }),
        &mut v,
        if percent {
            Scrub::fine(0.25, 1)
        } else {
            Scrub::whole(0.5).range(0.0..=f64::MAX)
        },
        |d| match word {
            Some(w) => d.custom_formatter(move |_, _| w.into()),
            None => mixed_if(d.custom_formatter(ui::number(2)), number.is_none()),
        },
    );
    let mut picked = None;
    // **Anchored on the unit, right edges together** (§15 D907). It hung off the
    // number's response, whose rect starts at the prefix, so a click on the unit at
    // the field's right end opened the menu under the letter at its left — and, in
    // the Transform card's W, over the field beside it.
    if let Some(unit) = unit {
        egui::Popup::menu(&unit)
            .align(egui::RectAlign::BOTTOM_END)
            .show(|ui| {
                ui::menu_rows(ui);
                // The dropdowns' row padding — the theme's `button_padding.x`, which
                // a `ComboBox`'s rows inherit and `Popup::menu`'s own style cuts to
                // 2, leaving "px" touching the highlight's edges. `y` is one of the
                // two values `ui::MENU_ROW_H`'s floor is proved against
                // (`a_menu_rows_height_does_not_depend_on_its_state`).
                ui.spacing_mut().button_padding = egui::vec2(8.0, 2.0);
                for m in modes {
                    let row = ui
                        .selectable_label(mode == Some(*m), m.label())
                        .on_hover_text(m.describe());
                    if row.clicked() {
                        picked = Some(*m);
                    }
                }
            });
    }
    // Held until the valve's committing frame, not just the frame the number
    // moved on ([`edited`], §15 D885).
    let typed = edited(&resp, v != start).then_some(v);
    SizeEdit {
        resp,
        picked,
        typed,
    }
}

// --- a dropdown with pictures -----------------------------------------------

/// A dropdown whose face and rows lead with a layout [`Glyph`], with the CSS
/// property's name inside the face — the mockup's field, where the name is the
/// label and the picture says which value at a glance.
///
/// `shown` is `None` for *mixed*: "Mixed" on the face, nothing lit in the list —
/// the stroke-alignment combo's rule, which is that mixed is a report and not a
/// value. `auto` is an extra first row whose face reads `Auto · <inherited>`
/// (`align-self`), with the inherited value's picture.
#[allow(clippy::too_many_arguments)]
fn glyph_combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    salt: &str,
    label: &str,
    width: f32,
    shown: Option<Option<T>>,
    options: &[T],
    name: fn(T) -> &'static str,
    glyph: impl Fn(T) -> Glyph,
    orient: impl Fn(Glyph) -> Orient,
    auto: Option<T>,
    enabled: bool,
    hint: Option<&str>,
) -> Option<Option<T>> {
    let mut picked = None;
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
        ui.spacing_mut().button_padding.y = 0.0;
        let pad = ui.spacing().button_padding.x;
        let ink = if enabled {
            color::TEXT
        } else {
            theme::text::DISABLED
        };
        let label_font = egui::FontId::proportional(10.5);
        let value_font = egui::FontId::proportional(12.0);
        let label_w = ui
            .ctx()
            .fonts_mut(|f| f.layout_no_wrap(label.to_owned(), label_font.clone(), ink))
            .size()
            .x;
        // What the face shows: a value and its picture, or "Auto · Stretch" with the
        // inherited value's picture, or "Mixed" with none.
        let (face_text, face_glyph) = match shown {
            Some(Some(v)) => (name(v).to_owned(), Some(glyph(v))),
            Some(None) => match auto {
                Some(inherited) => (
                    format!("Auto · {}", name(inherited)),
                    Some(glyph(inherited)),
                ),
                None => (ui::MIXED_WORD.to_owned(), None),
            },
            None => (ui::MIXED_WORD.to_owned(), None),
        };
        let mut job = egui::text::LayoutJob::default();
        job.append(
            label,
            0.0,
            egui::TextFormat::simple(label_font, theme::text::FAINT),
        );
        job.append(
            &face_text,
            GLYPH_GAP * 2.0 + GLYPH_PT,
            egui::TextFormat::simple(value_font.clone(), ink),
        );
        let inner = ui::disable_unless(ui, enabled, |ui| {
            egui::ComboBox::from_id_salt(salt)
                .icon(ui::combo_chevron)
                .width(width)
                .height(260.0)
                .selected_text(job)
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui.spacing_mut().button_padding.y = 2.0;
                    let row_pad = ui.spacing().button_padding.x;
                    let rows = auto
                        .map(|_| None)
                        .into_iter()
                        .chain(options.iter().map(|o| Some(*o)));
                    for row in rows {
                        let mut text = egui::text::LayoutJob::default();
                        text.append(
                            row.map_or("Auto", name),
                            GLYPH_PT + GLYPH_GAP,
                            egui::TextFormat::simple(value_font.clone(), color::TEXT),
                        );
                        let lit = shown == Some(row);
                        let r = ui.selectable_label(lit, text);
                        if let Some(v) = row {
                            let g = glyph(v);
                            paint_glyph(
                                ui.painter(),
                                egui::pos2(
                                    r.rect.left() + row_pad + GLYPH_PT / 2.0,
                                    r.rect.center().y,
                                ),
                                GLYPH_PT,
                                g,
                                orient(g),
                                theme::text::DIM,
                            );
                        }
                        if r.clicked() {
                            picked = Some(row);
                        }
                    }
                })
                .response
        });
        let face = inner.inner.rect;
        if let Some(g) = face_glyph {
            paint_glyph(
                ui.painter(),
                egui::pos2(
                    face.left() + pad + label_w + GLYPH_GAP + GLYPH_PT / 2.0,
                    face.center().y,
                ),
                GLYPH_PT,
                g,
                orient(g),
                ink,
            );
        }
        if let Some(hint) = hint {
            ui.painter().text(
                egui::pos2(face.right() - 26.0, face.center().y),
                egui::Align2::RIGHT_CENTER,
                hint,
                egui::FontId::proportional(10.5),
                theme::text::DISABLED,
            );
        }
    });
    picked
}

/// A `display` cell: its picture and its word, centred together.
fn display_cell(p: &egui::Painter, i: usize, rect: egui::Rect, on: bool) {
    let (g, word) = [
        (Glyph::DisplayNone, "None"),
        (Glyph::DisplayFlex, "Flex"),
        (Glyph::DisplayGrid, "Grid"),
    ][i];
    // `grid` was drawn disabled until the grid step built it (§15 D920).
    let ink = if on { color::TEXT } else { theme::text::DIM };
    let galley = p.layout_no_wrap(
        word.to_owned(),
        egui::FontId::proportional(ui::SEGMENT_LABEL_PT + 1.0),
        ink,
    );
    const G: f32 = 12.0;
    const GAP: f32 = 5.0;
    let x = rect.center().x - (G + GAP + galley.size().x) / 2.0;
    paint_glyph(
        p,
        egui::pos2(x + G / 2.0, rect.center().y),
        G,
        g,
        Orient::default(),
        ink,
    );
    p.galley(
        egui::pos2(x + G + GAP, rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
}

// --- why a layer is out of the flow ------------------------------------------------

/// Why a child of a container is not laid out by it — `container::in_flow`'s three
/// answers, in the order the card names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutOfFlow {
    /// Pinned by insets: `position: absolute` (§15 D874).
    Absolute,
    /// Hidden: CSS's `display: none`, ruled in §15 D881.
    Hidden,
    /// A mask: it clips its siblings and is never laid out.
    Mask,
}

// --- the cards ----------------------------------------------------------------

impl OndinApp {
    /// The selected frames and groups — every layer that can be a container
    /// (`container::is_container`), in document order. Filtered rather than
    /// all-or-nothing, `frame_subjects`' rule.
    pub(super) fn container_subjects(&self) -> Vec<NodeId> {
        let doc = &self.session.doc;
        ondin_core::build::in_document_order(doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| {
                doc.get(*id)
                    .is_some_and(|n| container::is_container(n.kind()))
            })
            .collect()
    }

    /// The selected layers whose parent lays its children out — a frame or a group
    /// with a `display` — in document order, in the flow or out of it.
    pub(super) fn item_subjects(&self) -> Vec<NodeId> {
        let doc = &self.session.doc;
        ondin_core::build::in_document_order(doc, self.session.selection.ids())
            .into_iter()
            .filter(|id| {
                doc.get(*id)
                    .and_then(|n| n.parent())
                    .and_then(|p| doc.get(p))
                    .is_some_and(|p| p.display().is_some() && container::is_container(p.kind()))
            })
            .collect()
    }

    /// Why `id` is out of its container's flow, or `None` for an item in it —
    /// through the preview, so a pin being dragged reads as the pin it will be.
    pub(super) fn out_of_flow(&self, id: NodeId) -> Option<OutOfFlow> {
        let node = self.session.display_node(id)?;
        if node.insets().is_authored() {
            Some(OutOfFlow::Absolute)
        } else if !node.visible() {
            Some(OutOfFlow::Hidden)
        } else if self.session.doc.get(id).is_some_and(|n| n.mask()) {
            Some(OutOfFlow::Mask)
        } else {
            None
        }
    }

    /// The **Container** card: `display`, and under `flex` or `grid` everything
    /// that layout has (§15 D878, D920). Drawn for any selection with a frame or a
    /// group in it; every edit applies to each of them.
    pub(super) fn inspector_container(&mut self, ui: &mut egui::Ui) {
        let subjects = self.container_subjects();
        if subjects.is_empty() {
            return;
        }
        let displays: Vec<Option<Display>> = subjects
            .iter()
            .map(|id| self.session.display_node(*id).and_then(|n| n.display()))
            .collect();
        let mode = shared(&displays, |d| match d {
            None => 0,
            Some(Display::Flex(_)) => 1,
            Some(Display::Grid(_)) => 2,
        });
        let flexes: Vec<Flex> = displays
            .iter()
            .filter_map(|d| match d {
                Some(Display::Flex(f)) => Some(*f),
                _ => None,
            })
            .collect();
        let grids: Vec<Grid> = displays
            .iter()
            .filter_map(|d| match d {
                Some(Display::Grid(g)) => Some(g.clone()),
                _ => None,
            })
            .collect();
        self.panel(ui, "Container", None, |app, ui| {
            let full = ui.available_width();
            // Nothing raised while the selection disagrees — `segmented`'s mixed
            // reading, an index past the last cell — and the first cell's picture
            // traded for `ui::segment_mixed`'s dash, the Type panel's convention
            // and one of D130's two dashes; blank read as a bug (§15 D892).
            if let Some(i) = ui::segmented_tipped(
                ui,
                full,
                ui::SEGMENT_CELL_H,
                3,
                mode.unwrap_or(3),
                |_| true,
                |i| DISPLAY_TIPS[i],
                |p, i, r, on| {
                    if mode.is_none() && i == 0 {
                        ui::segment_mixed(p, r);
                    } else {
                        display_cell(p, i, r, on);
                    }
                },
            ) {
                app.set_display(&subjects, i);
            }
            let laid: Vec<Display> = displays.iter().flatten().cloned().collect();
            match mode {
                Some(1) => app.flex_rows(ui, &subjects, &flexes, &laid),
                Some(2) => app.grid_container_rows(ui, &subjects, &grids, &laid),
                _ => {}
            }
        });
    }

    /// Give every subject the layout of `display` cell `to` — none, flex or grid.
    ///
    /// **Taking it away keeps every child where it is drawn** — its used transform
    /// and size written back as its own (`inspector::baked_ops`), and a frame that
    /// was hugging its children keeps the size it hugged to. Without that, `none`
    /// would send every child back to wherever its stored transform last had it,
    /// which is a place it has not been drawn since the layout was set: the
    /// Position card's unpin rule, for the same reason (§15 D878).
    /// `build::keep_flex_sizes` lets the children's transforms stand, because the
    /// same transaction relays their parent (§15 D877's amendment).
    ///
    /// **Switching between flex and grid keeps the padding and the gaps** (§15
    /// D920) — the properties the two share by name — and starts the rest at CSS's
    /// defaults; the children's item records keep both layouts' properties, so what
    /// they had under the other layout is waiting for them.
    fn set_display(&mut self, subjects: &[NodeId], to: usize) {
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let mut ops = Vec::new();
        for id in subjects {
            let Some(node) = doc.get(*id) else {
                continue;
            };
            let was = node.display();
            let kept = |mut d: Display| {
                if let Some(w) = was {
                    *d.padding_mut() = w.padding();
                    let (c, r) = w.gaps();
                    let (mc, mr) = d.gaps_mut();
                    (*mc, *mr) = (c, r);
                }
                d
            };
            let now = match (to, was) {
                (1, Some(Display::Flex(_))) | (2, Some(Display::Grid(_))) | (0, None) => continue,
                (1, _) => Some(kept(Display::Flex(Flex::default()))),
                (2, _) => Some(kept(Display::Grid(Grid::default()))),
                _ => None,
            };
            match (now, was.is_some()) {
                (Some(display), _) => ops.push(Operation::SetDisplay {
                    id: *id,
                    display: Some(display),
                }),
                (None, true) => {
                    for child in node.children() {
                        if ondin_core::build::is_flex_item(doc, *child)
                            && let (Some(c), Some(local), Some(kind)) = (
                                doc.get(*child),
                                res.used_local(doc, *child),
                                res.used_kind(doc, *child),
                            )
                        {
                            ops.extend(baked_ops(*child, c, local, kind));
                        }
                    }
                    // A frame hugging its children keeps the size it hugged to.
                    if let Some(kind @ NodeKind::Artboard { .. }) = res.used_kind(doc, *id) {
                        ops.extend(
                            baked_ops(*id, node, node.transform(), kind)
                                .into_iter()
                                .filter(|op| matches!(op, Operation::SetGeometry { .. })),
                        );
                        let mut item = *node.item();
                        for d in [&mut item.width, &mut item.height] {
                            if *d == Dimension::FitContent {
                                *d = Dimension::Auto;
                            }
                        }
                        if item != *node.item() {
                            ops.push(Operation::SetLayoutItem { id: *id, item });
                        }
                    }
                    ops.push(Operation::SetDisplay {
                        id: *id,
                        display: None,
                    });
                }
                _ => {}
            }
        }
        self.commit_edit(Transaction(ops));
    }

    /// Every subject's flex layout with `f` applied, as the ops that change one —
    /// **from the committed document**, D51's rule for a transaction a valve both
    /// previews and commits.
    fn flex_tx(&self, subjects: &[NodeId], f: impl Fn(&mut Flex)) -> Transaction {
        self.display_tx(subjects, |d| {
            if let Display::Flex(flex) = d {
                f(flex);
            }
        })
    }

    /// Every subject's layout, flex or grid, with `f` applied — [`Self::flex_tx`]
    /// for what both layouts share (the gaps, the padding) and for grid's own rows
    /// (§15 D920). From the committed document, D51's rule.
    fn display_tx(&self, subjects: &[NodeId], f: impl Fn(&mut Display)) -> Transaction {
        Transaction(
            subjects
                .iter()
                .filter_map(|id| {
                    let was = self.session.doc.get(*id)?.display()?;
                    let mut now = was.clone();
                    f(&mut now);
                    (&now != was).then_some(Operation::SetDisplay {
                        id: *id,
                        display: Some(now),
                    })
                })
                .collect(),
        )
    }

    /// The rows under `display: flex` — the mockup's order: direction and wrap,
    /// the three alignments, the gaps, the padding. `laid` is every subject's
    /// layout, for the rows flex shares with grid.
    fn flex_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        flexes: &[Flex],
        laid: &[Display],
    ) {
        let Some(&first) = flexes.first() else {
            return;
        };
        let full = ui.available_width();
        let gap = ui::CARD_COL_GAP;
        let direction = shared(flexes, |f| f.direction);
        let wrap = shared(flexes, |f| f.wrap);
        // The pictures turn with the layout they describe, read off the first
        // subject's when the selection disagrees.
        let orient = |g: Glyph| Orient::of(g, &first);
        let wraps = wrap != Some(FlexWrap::NoWrap);

        // --- direction and wrap ---------------------------------------------
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let cell = (full - gap) / 7.0;
            let arrows = [
                icon::ARROW_RIGHT,
                icon::ARROW_LEFT,
                icon::ARROW_DOWN,
                icon::ARROW_UP,
            ];
            let at = direction
                .and_then(|d| DIRECTIONS.iter().position(|x| *x == d))
                .unwrap_or(DIRECTIONS.len());
            if let Some(i) = ui::segmented_tipped(
                ui,
                cell * 4.0,
                ui::SEGMENT_CELL_H,
                4,
                at,
                |_| true,
                |i| DIRECTION_TIPS[i],
                |p, i, r, on| match direction {
                    None if i == 0 => ui::segment_mixed(p, r),
                    _ => ui::segment_glyph(p, r, arrows[i], on),
                },
            ) {
                let tx = self.flex_tx(subjects, |f| f.direction = DIRECTIONS[i]);
                self.commit_edit(tx);
            }
            let turns = [
                icon::ARROW_LINE_RIGHT,
                icon::ARROW_U_DOWN_LEFT,
                icon::ARROW_U_UP_LEFT,
            ];
            let at = wrap
                .and_then(|w| WRAPS.iter().position(|x| *x == w))
                .unwrap_or(WRAPS.len());
            if let Some(i) = ui::segmented_tipped(
                ui,
                cell * 3.0,
                ui::SEGMENT_CELL_H,
                3,
                at,
                |_| true,
                |i| WRAP_TIPS[i],
                |p, i, r, on| match wrap {
                    None if i == 0 => ui::segment_mixed(p, r),
                    _ => ui::segment_glyph(p, r, turns[i], on),
                },
            ) {
                let tx = self.flex_tx(subjects, |f| f.wrap = WRAPS[i]);
                self.commit_edit(tx);
            }
        });

        // --- the alignments -------------------------------------------------
        if let Some(Some(j)) = glyph_combo(
            ui,
            "flex-justify",
            "Justify content",
            full,
            shared(flexes, |f| Some(f.justify_content)),
            &JUSTIFY,
            justify_name,
            Glyph::Justify,
            orient,
            None,
            true,
            None,
        ) {
            let tx = self.flex_tx(subjects, |f| f.justify_content = j);
            self.commit_edit(tx);
        }
        if let Some(Some(a)) = glyph_combo(
            ui,
            "flex-align-items",
            "Align items",
            full,
            shared(flexes, |f| Some(f.align_items)),
            &ALIGN,
            align_name,
            Glyph::Align,
            orient,
            None,
            true,
            None,
        ) {
            let tx = self.flex_tx(subjects, |f| f.align_items = a);
            self.commit_edit(tx);
        }
        // **Dimmed in place rather than removed** while nothing wraps — the
        // mockup's note: toggling wrap must not shift the card under the pointer.
        if let Some(Some(c)) = glyph_combo(
            ui,
            "flex-align-content",
            "Align content",
            full,
            shared(flexes, |f| Some(f.align_content)),
            &CONTENT,
            content_name,
            Glyph::Content,
            orient,
            None,
            wraps,
            (!wraps).then_some("Needs wrap"),
        ) {
            let tx = self.flex_tx(subjects, |f| f.align_content = c);
            self.commit_edit(tx);
        }

        // --- the gaps: between items first ------------------------------------
        // **The first field is always the gap between items** — `column-gap` in a
        // row, `row-gap` in a column — and a selection that disagrees about
        // direction reads in row order (the mockup's two rules). Between lines
        // means nothing on one line.
        let column = direction.is_some_and(|d| !d.is_row());
        self.gap_row(ui, subjects, laid, !column, wraps, full);

        // --- the padding ------------------------------------------------------
        self.padding_rows(ui, subjects, laid, full);
    }

    /// The two gaps, side by side — `column-gap` first when `column_first`, and
    /// the second field dimmed unless `second_live`. Flex puts the gap between its
    /// items first and dims the one between lines on one line; grid shows both,
    /// columns first (§15 D920).
    fn gap_row(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        laid: &[Display],
        column_first: bool,
        second_live: bool,
        full: f32,
    ) {
        let Some(first) = laid.first() else {
            return;
        };
        let gap = ui::CARD_COL_GAP;
        let half = egui::vec2((full - gap) / 2.0, ui::CONTROL_H);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for is_first in [true, false] {
                let is_column_gap = is_first == column_first;
                let (glyph, word) = if is_column_gap {
                    (icon::ARROWS_OUT_LINE_HORIZONTAL, "Column gap")
                } else {
                    (icon::ARROWS_OUT_LINE_VERTICAL, "Row gap")
                };
                let get = |d: &Display| {
                    let (c, r) = d.gaps();
                    if is_column_gap { c } else { r }
                };
                let shown = shared(laid, get);
                let live = is_first || second_live;
                ui::disable_unless(ui, live, |ui| {
                    let mut v = shown.unwrap_or_else(|| get(first));
                    let start = v;
                    let (resp, _) = ui::value_field_suffixed(
                        ui,
                        half,
                        Prefix::Icon(glyph),
                        Some(Suffix {
                            text: word,
                            clickable: false,
                            tooltip: "",
                        }),
                        &mut v,
                        Scrub::whole(0.5).range(0.0..=f64::MAX),
                        |d| mixed_if(d.custom_formatter(ui::number(2)), shown.is_none()),
                    );
                    let tx = if !edited(&resp, v != start) {
                        Transaction(Vec::new())
                    } else {
                        self.display_tx(subjects, |d| {
                            let (c, r) = d.gaps_mut();
                            *(if is_column_gap { c } else { r }) = v.max(0.0);
                        })
                    };
                    self.edit_valve(&resp, tx);
                });
            }
        });
    }

    /// Padding: left-and-right and top-and-bottom as two fields, opening to the
    /// four sides — L R over T B, the Position card's order — with the button at
    /// the end of the row. **Open by itself whenever two opposite sides differ**, so
    /// the two fields never have to stand for a pair they do not describe. Either
    /// layout's (§15 D920).
    fn padding_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        laid: &[Display],
        full: f32,
    ) {
        let paddings: Vec<[f64; 4]> = laid.iter().map(Display::padding).collect();
        const TOP: usize = 0;
        const RIGHT: usize = 1;
        const BOTTOM: usize = 2;
        const LEFT: usize = 3;
        let gap = ui::CARD_COL_GAP;
        let side = ui::CONTROL_H;
        let uneven = paddings
            .iter()
            .any(|p| p[LEFT] != p[RIGHT] || p[TOP] != p[BOTTOM]);
        let key = egui::Id::new(("flex-padding-sides", subjects.first().copied()));
        let asked = ui.ctx().data(|d| d.get_temp::<bool>(key).unwrap_or(false));
        let open = asked || uneven;
        let pair = egui::vec2((full - side - gap * 2.0) / 2.0, side);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (sides, glyph, word) in [
                (
                    [LEFT, RIGHT],
                    icon::ARROWS_IN_LINE_HORIZONTAL,
                    "Left and right padding",
                ),
                (
                    [TOP, BOTTOM],
                    icon::ARROWS_IN_LINE_VERTICAL,
                    "Top and bottom padding",
                ),
            ] {
                let shown = shared(&paddings, |p| {
                    (p[sides[0]] == p[sides[1]]).then_some(p[sides[0]])
                })
                .flatten();
                let mut v = shown.unwrap_or(first_or_zero(&paddings, sides[0]));
                let start = v;
                let resp = ui::value_field(
                    ui,
                    pair,
                    Prefix::Icon(glyph),
                    &mut v,
                    Scrub::whole(0.5).range(0.0..=f64::MAX),
                    |d| mixed_if(d.custom_formatter(ui::number(2)), shown.is_none()),
                )
                .on_hover_text(word);
                let tx = if !edited(&resp, v != start) {
                    Transaction(Vec::new())
                } else {
                    self.display_tx(subjects, |d| {
                        for s in sides {
                            d.padding_mut()[s] = v.max(0.0);
                        }
                    })
                };
                self.edit_valve(&resp, tx);
            }
            let state = if uneven {
                ui::FieldButton::Set
            } else {
                ui::FieldButton::on_if(open)
            };
            if ui::field_button(ui, icon::SQUARE, side, 15.0, state)
                .on_hover_text(if uneven {
                    "Padding per side — the sides differ, so they stay open"
                } else if open {
                    "Padding per side"
                } else {
                    "Set padding per side"
                })
                .clicked()
                && !uneven
            {
                ui.ctx().data_mut(|d| d.insert_temp(key, !asked));
            }
        });
        if !open {
            return;
        }
        let half = egui::vec2((full - gap) / 2.0, side);
        for pair in [[(LEFT, "L"), (RIGHT, "R")], [(TOP, "T"), (BOTTOM, "B")]] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (s, letter) in pair {
                    let shown = shared(&paddings, |p| p[s]);
                    let mut v = shown.unwrap_or(first_or_zero(&paddings, s));
                    let start = v;
                    let resp = ui::value_field(
                        ui,
                        half,
                        Prefix::Text(letter),
                        &mut v,
                        Scrub::whole(0.5).range(0.0..=f64::MAX),
                        |d| mixed_if(d.custom_formatter(ui::number(2)), shown.is_none()),
                    );
                    let tx = if !edited(&resp, v != start) {
                        Transaction(Vec::new())
                    } else {
                        self.display_tx(subjects, |d| d.padding_mut()[s] = v.max(0.0))
                    };
                    self.edit_valve(&resp, tx);
                }
            });
        }
    }

    /// The **Item** card: a layer's place in its parent's flow (§15 D878) — or, out
    /// of it, why, and the button that brings it back.
    pub(super) fn inspector_item(&mut self, ui: &mut egui::Ui) {
        let subjects = self.item_subjects();
        let Some(&anchor) = subjects.first() else {
            return;
        };
        let flowing: Vec<NodeId> = subjects
            .iter()
            .copied()
            .filter(|id| self.out_of_flow(*id).is_none())
            .collect();
        self.panel(ui, "Item", None, |app, ui| {
            if flowing.is_empty() {
                if let Some(why) = app.out_of_flow(anchor) {
                    app.out_of_flow_block(ui, &subjects, anchor, why);
                }
                return;
            }
            if flowing.len() < subjects.len() {
                ui.label(
                    egui::RichText::new(format!(
                        "{} of these are out of the flow and keep their place.",
                        subjects.len() - flowing.len()
                    ))
                    .size(11.0)
                    .color(theme::text::FAINT),
                );
            }
            app.item_rows(ui, &flowing);
        });
    }

    /// Why the layer is out of the flow, in words, and the one button that puts it
    /// back — each reason with its own (the mockup's 07). The button acts on every
    /// subject out for the same reason.
    fn out_of_flow_block(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        anchor: NodeId,
        why: OutOfFlow,
    ) {
        // **Plain words, not CSS's** (§15 D889): the title was `position: absolute`
        // and the sentence named "insets" and the Position card, which read as
        // jargon about jargon. What the user needs is what holds the layer and
        // what the layout does about it.
        let (glyph, title, body, button, button_glyph) = match why {
            OutOfFlow::Absolute => (
                icon::CROSSHAIR_SIMPLE,
                "Absolutely positioned".to_owned(),
                format!(
                    "{}, so it sits outside the layout.",
                    pinned_edges(
                        &self
                            .session
                            .display_node(anchor)
                            .map(|n| n.insets())
                            .unwrap_or_default()
                    )
                ),
                "Unpin and return to the layout",
                icon::X,
            ),
            OutOfFlow::Hidden => (
                icon::EYE_SLASH,
                "Hidden".to_owned(),
                "Hidden layers take no space in the layout.".to_owned(),
                "Show layer",
                icon::EYE,
            ),
            OutOfFlow::Mask => (
                icon::CIRCLE_HALF,
                "Mask".to_owned(),
                "Masks clip their siblings and are never laid out.".to_owned(),
                "Release mask",
                icon::CIRCLE_HALF,
            ),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 9.0;
            ui.label(theme::icon_text(glyph, 15.0, theme::text::DIM));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(
                    egui::RichText::new(title)
                        .size(12.0)
                        .color(theme::text::STRONG),
                );
                ui.add(
                    egui::Label::new(egui::RichText::new(body).size(11.0).color(theme::text::DIM))
                        .wrap(),
                );
            });
        });
        let size = egui::vec2(ui.available_width(), ui::CONTROL_H);
        if ui::action_button(ui, button_glyph, button, ui::FieldButton::Off, size).clicked() {
            self.back_into_flow(subjects, why);
        }
    }

    /// The out-of-flow block's button: every subject out for `why`, back in the
    /// flow as one commit — pinned layers unpinned where they are drawn (their
    /// transform and size kept, `inspector::baked_ops`), hidden ones shown, masks
    /// released. Each rejoins at its own place among its siblings.
    fn back_into_flow(&mut self, subjects: &[NodeId], why: OutOfFlow) {
        let same: Vec<NodeId> = subjects
            .iter()
            .copied()
            .filter(|id| self.out_of_flow(*id) == Some(why))
            .collect();
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let ops: Vec<Operation> = match why {
            // Where it is drawn, kept — then the insets go. `keep_flex_sizes` lets
            // the transform stand, because the same edit changes its flow (§15
            // D877's amendment).
            OutOfFlow::Absolute => same
                .iter()
                .flat_map(|id| {
                    let mut ops = match (
                        doc.get(*id),
                        res.used_local(doc, *id),
                        res.used_kind(doc, *id),
                    ) {
                        (Some(n), Some(local), Some(kind)) => baked_ops(*id, n, local, kind),
                        _ => Vec::new(),
                    };
                    ops.push(Operation::SetInsets {
                        id: *id,
                        insets: ondin_core::Insets::default(),
                    });
                    ops
                })
                .collect(),
            OutOfFlow::Hidden => same
                .iter()
                .map(|id| Operation::SetVisible {
                    id: *id,
                    visible: true,
                })
                .collect(),
            OutOfFlow::Mask => same
                .iter()
                .map(|id| Operation::SetMask {
                    id: *id,
                    mask: false,
                })
                .collect(),
        };
        self.commit_edit(Transaction(ops));
    }

    /// Every flowing subject's item properties with `f` applied, as the ops that
    /// change one — from the committed document, [`Self::flex_tx`]'s rule.
    fn item_tx(&self, subjects: &[NodeId], f: impl Fn(&mut LayoutItem)) -> Transaction {
        Transaction(
            subjects
                .iter()
                .filter_map(|id| {
                    let was = *self.session.doc.get(*id)?.item();
                    let mut now = was;
                    f(&mut now);
                    (now != was).then_some(Operation::SetLayoutItem { id: *id, item: now })
                })
                .collect(),
        )
    }

    /// The flowing subjects' rows: grow and shrink, basis and `align-self` in a
    /// flex container, or the lines and both self-alignments in a grid (§15
    /// D920); the four limits behind a disclosure; and the receipt of a resize
    /// that changed any of them (§15 D880).
    fn item_rows(&mut self, ui: &mut egui::Ui, subjects: &[NodeId]) {
        let items: Vec<LayoutItem> = subjects
            .iter()
            .filter_map(|id| self.session.display_node(*id).map(|n| n.item()))
            .collect();
        let Some(&first) = items.first() else {
            return;
        };
        let full = ui.available_width();
        let parent = self.session.doc.get(subjects[0]).and_then(|n| n.parent());
        let parent_display = parent
            .and_then(|p| self.session.display_node(p))
            .and_then(|n| n.display());
        let parent_flex = match &parent_display {
            Some(Display::Flex(f)) => *f,
            _ => Flex::default(),
        };
        let parent_grid = match parent_display {
            Some(Display::Grid(g)) => Some(g),
            _ => None,
        };
        // What the last resize flipped, for the subjects in hand.
        let receipt: Vec<(LayoutItem, LayoutItem)> = self
            .session
            .flex_receipt()
            .map(|r| {
                r.held
                    .iter()
                    .filter(|(id, ..)| subjects.contains(id))
                    .map(|(_, was, now)| (*was, *now))
                    .collect()
            })
            .unwrap_or_default();
        let flipped_grow = receipt.iter().any(|(w, n)| w.grow != n.grow);
        let flipped_shrink = receipt.iter().any(|(w, n)| w.shrink != n.shrink);
        let flipped_align = receipt.iter().any(|(w, n)| w.align_self != n.align_self);
        let flipped_justify = receipt
            .iter()
            .any(|(w, n)| w.justify_self != n.justify_self);

        // The first subject's drawn box, which a mode picked from a unit converts.
        let drawn = self.session.preview_local_box(subjects[0]);

        // **A grid item has its lines and its two self-alignments where a flex
        // item has grow, shrink, basis and `align-self`** (§15 D920); the limits
        // and the receipt below are both layouts'.
        if let Some(grid) = &parent_grid {
            self.grid_item_rows(
                ui,
                subjects,
                &items,
                grid,
                full,
                [flipped_justify, flipped_align],
            );
        } else {
            self.flex_item_rows(
                ui,
                subjects,
                &items,
                &parent_flex,
                parent,
                drawn,
                [flipped_grow, flipped_shrink, flipped_align],
            );
        }

        // --- min / max ----------------------------------------------------------
        self.limit_rows(ui, subjects, &items, first, parent, drawn, full);

        // --- the receipt --------------------------------------------------------
        if let Some((_, now)) = receipt.first() {
            let mut said = Vec::new();
            if flipped_grow {
                said.push(format!("flex grow to {}", ui::number(2)(now.grow, 0..=2)));
            }
            if flipped_shrink {
                said.push(format!(
                    "flex shrink to {}",
                    ui::number(2)(now.shrink, 0..=2)
                ));
            }
            // Mid-sentence, so lower-cased back from the menu's sentence case —
            // grid's names for a grid item (§15 D920).
            let name = |a: AlignItems| match (&parent_grid, a) {
                (Some(_), AlignItems::Start) => "start",
                (Some(_), AlignItems::End) => "end",
                _ => align_name(a),
            };
            if flipped_justify && let Some(a) = now.justify_self {
                said.push(format!("justify self to {}", name(a).to_lowercase()));
            }
            if flipped_align && let Some(a) = now.align_self {
                said.push(format!("align self to {}", name(a).to_lowercase()));
            }
            let list = match said.len() {
                0 => return,
                1 => said[0].clone(),
                n => format!("{} and {}", said[..n - 1].join(", "), said[n - 1]),
            };
            let mut undo = false;
            egui::Frame::new()
                .fill(color::ACCENT.gamma_multiply(0.14))
                .corner_radius(egui::CornerRadius::same(ui::BUTTON_R))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Resizing set {list}."))
                                    .size(11.0)
                                    .color(theme::text::STRONG),
                            )
                            .wrap(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                            undo = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new("Undo")
                                            .size(11.0)
                                            .color(color::ACCENT_300),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text("Undo the resize — the flips are what made it hold")
                                .clicked();
                        });
                    });
                });
            if undo {
                self.undo();
            }
        }
    }

    /// A flex item's rows: grow and shrink, basis and `align-self` (§15 D878),
    /// each accented while the last resize's receipt names it (§15 D880) —
    /// `flipped` is grow, shrink, align.
    #[allow(clippy::too_many_arguments)]
    fn flex_item_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        items: &[LayoutItem],
        parent_flex: &Flex,
        parent: Option<NodeId>,
        drawn: Option<ondin_core::kurbo::Rect>,
        flipped: [bool; 3],
    ) {
        let [flipped_grow, flipped_shrink, flipped_align] = flipped;
        let Some(&first) = items.first() else {
            return;
        };
        let full = ui.available_width();
        let gap = ui::CARD_COL_GAP;
        let accent = |ui: &egui::Ui, rect: egui::Rect, on: bool| {
            if on {
                ui.painter().rect_stroke(
                    rect,
                    egui::CornerRadius::same(ui::BUTTON_R),
                    egui::Stroke::new(1.0, color::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
        };

        // --- grow and shrink ----------------------------------------------------
        let half = egui::vec2((full - gap) / 2.0, ui::CONTROL_H);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (label, is_grow, flipped) in [
                ("Flex grow", true, flipped_grow),
                ("Flex shrink", false, flipped_shrink),
            ] {
                let get = |i: &LayoutItem| if is_grow { i.grow } else { i.shrink };
                let shown = shared(items, get);
                let mut v = shown.unwrap_or_else(|| get(&first));
                let start = v;
                let at = egui::Rect::from_min_size(ui.cursor().min, half);
                let resp = ui::value_field(
                    ui,
                    half,
                    Prefix::Label(label),
                    &mut v,
                    Scrub::fine(0.05, 1).range(0.0..=f64::MAX),
                    |d| mixed_if(d.custom_formatter(ui::number(2)), shown.is_none()),
                );
                accent(ui, at, flipped);
                let tx = if !edited(&resp, v != start) {
                    Transaction(Vec::new())
                } else {
                    self.item_tx(subjects, |i| {
                        if is_grow {
                            i.grow = v.max(0.0);
                        } else {
                            i.shrink = v.max(0.0);
                        }
                    })
                };
                self.edit_valve(&resp, tx);
            }
        });

        // --- basis ------------------------------------------------------------
        let row = parent_flex.direction.is_row();
        let main = drawn.map(|b| if row { b.width() } else { b.height() });
        let main_extent = parent.and_then(|p| self.content_extent(p, row));
        self.dimension_row(
            ui,
            subjects,
            items,
            "Flex basis",
            full,
            |i| i.basis,
            |i, d| i.basis = d,
            main,
            main_extent,
            &[SizeMode::Auto, SizeMode::Px, SizeMode::Percent],
        );

        // --- align-self -------------------------------------------------------
        let at = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(full, ui::CONTROL_H));
        if let Some(pick) = glyph_combo(
            ui,
            "flex-align-self",
            "Align self",
            full,
            shared(items, |i| i.align_self),
            &ALIGN,
            align_name,
            Glyph::Align,
            |g| Orient::of(g, parent_flex),
            Some(parent_flex.align_items),
            true,
            None,
        ) {
            let tx = self.item_tx(subjects, |i| i.align_self = pick);
            self.commit_edit(tx);
        }
        accent(ui, at, flipped_align);
    }

    /// The four size limits behind their disclosure — either layout's item
    /// (§15 D878, D920).
    #[allow(clippy::too_many_arguments)]
    fn limit_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        items: &[LayoutItem],
        first: LayoutItem,
        parent: Option<NodeId>,
        drawn: Option<ondin_core::kurbo::Rect>,
        full: f32,
    ) {
        let set = [
            first.min_width,
            first.max_width,
            first.min_height,
            first.max_height,
        ]
        .iter()
        .filter(|d| **d != Dimension::Auto)
        .count();
        let key = egui::Id::new("flex-item-limits");
        let open = ui.ctx().data(|d| d.get_temp::<bool>(key).unwrap_or(false));
        let row = ui.horizontal(|ui| {
            ui.label(theme::icon_text(
                if open {
                    icon::CARET_DOWN
                } else {
                    icon::CARET_RIGHT
                },
                12.0,
                theme::text::DIM,
            ));
            ui.label(
                egui::RichText::new("Min / max")
                    .size(11.0)
                    .color(theme::text::DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(match set {
                        0 => "All auto".to_owned(),
                        n => format!("{n} set"),
                    })
                    .size(10.5)
                    .color(theme::text::FAINT),
                );
            });
        });
        let hit = ui.interact(
            row.response.rect,
            egui::Id::new("flex-item-limits-row"),
            egui::Sense::click(),
        );
        if hit.clicked() {
            ui.ctx().data_mut(|d| d.insert_temp(key, !open));
        }
        if open {
            let limits: [Limit; 4] = [
                ("Min width", true, |i| i.min_width, |i, d| i.min_width = d),
                ("Max width", true, |i| i.max_width, |i, d| i.max_width = d),
                (
                    "Min height",
                    false,
                    |i| i.min_height,
                    |i, d| i.min_height = d,
                ),
                (
                    "Max height",
                    false,
                    |i| i.max_height,
                    |i, d| i.max_height = d,
                ),
            ];
            for (label, horizontal, get, put) in limits {
                let along = drawn.map(|b| if horizontal { b.width() } else { b.height() });
                let extent = parent.and_then(|p| self.content_extent(p, horizontal));
                self.dimension_row(
                    ui,
                    subjects,
                    items,
                    label,
                    full,
                    get,
                    put,
                    along,
                    extent,
                    &[SizeMode::Auto, SizeMode::Px, SizeMode::Percent],
                );
            }
        }
    }

    /// The content box of container `id` along one axis — its drawn size less its
    /// padding — which is what a CSS percentage of it is a percentage of.
    pub(super) fn content_extent(&self, id: NodeId, horizontal: bool) -> Option<f64> {
        let node = self.session.display_node(id)?;
        let size = match node.kind() {
            NodeKind::Artboard { size } => *size,
            _ => self.session.preview_frame(id)?,
        };
        let pad = match node.display().map(|d| d.padding()) {
            Some(p) if horizontal => p[1] + p[3],
            Some(p) => p[0] + p[2],
            None => 0.0,
        };
        let extent = if horizontal { size.width } else { size.height } - pad;
        (extent > 0.0).then_some(extent)
    }

    /// One [`size_field`] row over a basis or a limit, full width: typing writes px
    /// (or % while the mode is %), and a mode picked from the unit converts what is
    /// drawn — `drawn` px, or `drawn` as a percentage of `extent` — so switching
    /// the unit moves nothing.
    #[allow(clippy::too_many_arguments)]
    fn dimension_row(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        items: &[LayoutItem],
        label: &'static str,
        width: f32,
        get: impl Fn(&LayoutItem) -> Dimension,
        put: impl Fn(&mut LayoutItem, Dimension) + Copy,
        drawn: Option<f64>,
        extent: Option<f64>,
        modes: &[SizeMode],
    ) {
        let shown = shared(items, |i| get(i));
        let mode = shown.map(plain_mode);
        let number = match shown {
            Some(Dimension::Px(v) | Dimension::Percent(v)) => Some(v),
            _ => None,
        };
        let edit = size_field(
            ui,
            egui::vec2(width, ui::CONTROL_H),
            Prefix::Label(label),
            mode,
            number,
            // Under a keyword, the size drawn, so a scrub starts from it.
            drawn.unwrap_or(0.0),
            modes,
        );
        if let Some(m) = edit.picked {
            let to = match m {
                SizeMode::Auto => Some(Dimension::Auto),
                SizeMode::FitContent => Some(Dimension::FitContent),
                SizeMode::Px => drawn.map(Dimension::Px),
                SizeMode::Percent => drawn
                    .zip(extent)
                    .map(|(d, e)| Dimension::Percent((d / e * 1000.0).round() / 10.0)),
            };
            if let Some(to) = to {
                let tx = self.item_tx(subjects, |i| put(i, to));
                self.commit_edit(tx);
            }
            return;
        }
        let tx = match edit.typed {
            Some(v) => {
                let to = if mode == Some(SizeMode::Percent) {
                    Dimension::Percent(v)
                } else {
                    Dimension::Px(v.max(0.0))
                };
                self.item_tx(subjects, |i| put(i, to))
            }
            None => Transaction(Vec::new()),
        };
        self.edit_valve(&edit.resp, tx);
    }

    /// W's or H's sizing mode for layer `id` from the Transform card — the ops a
    /// mode picked from the unit's menu writes (§15 D879). Each keeps the layer at
    /// the size it is drawn: `px` fixes the drawn size, `%` converts it against the
    /// container's content box, and a keyword hands the size to the layout.
    pub(super) fn size_mode_tx(
        &self,
        id: NodeId,
        horizontal: bool,
        mode: SizeMode,
        drawn: Size,
    ) -> Transaction {
        let doc = &self.session.doc;
        let Some(node) = doc.get(id) else {
            return Transaction(Vec::new());
        };
        let mut item = *node.item();
        let along = if horizontal {
            drawn.width
        } else {
            drawn.height
        };
        let mut ops = Vec::new();
        let to = match (mode, node.kind()) {
            // A group's px is its box; anything else's is its stored size, written
            // where it is drawn, with the keyword back to `auto`.
            (SizeMode::Px, NodeKind::Group) => Dimension::Px(along),
            (SizeMode::Px, _) => {
                if let Some(kind) = self.session.resolved.used_kind(doc, id) {
                    ops.extend(
                        baked_ops(id, node, node.transform(), kind)
                            .into_iter()
                            .filter(|op| matches!(op, Operation::SetGeometry { .. })),
                    );
                }
                Dimension::Auto
            }
            (SizeMode::Percent, _) => {
                let Some(extent) = node
                    .parent()
                    .and_then(|p| self.content_extent(p, horizontal))
                else {
                    return Transaction(Vec::new());
                };
                Dimension::Percent((along / extent * 1000.0).round() / 10.0)
            }
            (SizeMode::Auto, _) => Dimension::Auto,
            (SizeMode::FitContent, _) => Dimension::FitContent,
        };
        if horizontal {
            item.width = to;
        } else {
            item.height = to;
        }
        if item != *node.item() {
            ops.push(Operation::SetLayoutItem { id, item });
        }
        Transaction(ops)
    }
}

/// One of the Item card's four size limits: its CSS name, whether it is a width,
/// and how to read and write it on an item.
type Limit = (
    &'static str,
    bool,
    fn(&LayoutItem) -> Dimension,
    fn(&mut LayoutItem, Dimension),
);

/// `first`'s padding on `side`, or zero — the fallback a mixed field starts from.
fn first_or_zero(paddings: &[[f64; 4]], side: usize) -> f64 {
    paddings.first().map_or(0.0, |p| p[side])
}

/// "Pinned to its container's top and right edges" — which of a layer's edges
/// hold it, in the Item card's sentence about why it is out of the flow (§15
/// D889).
fn pinned_edges(insets: &ondin_core::Insets) -> String {
    let names: Vec<&str> = [
        (insets.top.is_some(), "top"),
        (insets.right.is_some(), "right"),
        (insets.bottom.is_some(), "bottom"),
        (insets.left.is_some(), "left"),
    ]
    .into_iter()
    .filter_map(|(on, n)| on.then_some(n))
    .collect();
    match names.as_slice() {
        [] => "Pinned inside its container".to_owned(),
        [one] => format!("Pinned to its container's {one} edge"),
        [init @ .., last] => format!(
            "Pinned to its container's {} and {last} edges",
            init.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Rect, RoundedRectRadii};
    use ondin_core::{Document, GeometryPatch, IdSource, Insets, LengthPct};

    /// A 400×200 frame laying out a row — gap 10, padding 20, CSS's default
    /// `align-items: stretch` — of two rects, 40 and 60 wide, both stored at
    /// (300, 150) so that anything placing them by their own transform shows.
    struct Scene {
        app: OndinApp,
        frame: NodeId,
        a: NodeId,
        b: NodeId,
    }

    fn scene() -> Scene {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(0xCA);
        let root = ids.mint();
        let (frame, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let rect = |id, index, w| Operation::CreateNode {
            id,
            parent: frame,
            index,
            kind: NodeKind::Rect {
                size: Size::new(w, 30.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate((300.0, 150.0))),
            name: None,
        };
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            rect(a, 0, 40.0),
            rect(b, 1, 60.0),
            Operation::SetDisplay {
                id: frame,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [20.0; 4],
                    ..Default::default()
                })),
            },
        ]))
        .unwrap();
        app.session.adopt_document(doc, None);
        Scene { app, frame, a, b }
    }

    fn drawn(app: &OndinApp, id: NodeId) -> Rect {
        app.session.resolved.world_bounds(id).expect("measured")
    }

    fn set_item(app: &mut OndinApp, id: NodeId, f: impl FnOnce(&mut LayoutItem)) {
        let mut item = *app.session.doc.get(id).unwrap().item();
        f(&mut item);
        app.session
            .commit(Transaction(vec![Operation::SetLayoutItem { id, item }]));
    }

    /// **The sizing menu offers only the modes that mean something different for
    /// the layer** (`size_modes`, §15 D879) — the table in its doc, cell by cell.
    /// A frame's `auto` and a shape's `fit-content` never appear, since each is the
    /// layer's `px`; `%` only where a laid-out parent gives it something to be a
    /// percentage of, and only for a kind the layout can resize.
    #[test]
    fn the_size_menu_offers_only_modes_that_differ_for_the_kind() {
        use SizeMode::*;
        let frame = NodeKind::Artboard {
            size: Size::new(1.0, 1.0),
        };
        let rect = NodeKind::Rect {
            size: Size::new(1.0, 1.0),
            corner_radii: RoundedRectRadii::default(),
        };
        let path = NodeKind::Path {
            path: Default::default(),
            corner_radii: Default::default(),
        };
        assert_eq!(size_modes(&frame, false, false), vec![Px], "no menu at all");
        assert_eq!(size_modes(&frame, true, false), vec![Px, FitContent]);
        assert_eq!(
            size_modes(&frame, true, true),
            vec![Px, FitContent, Percent]
        );
        assert_eq!(size_modes(&NodeKind::Group, true, false), vec![Px, Auto]);
        assert_eq!(
            size_modes(&NodeKind::Group, true, true),
            vec![Px, Auto, Percent]
        );
        assert_eq!(
            size_modes(&NodeKind::Group, false, true),
            vec![Px],
            "a group without a layout has no box to size"
        );
        assert_eq!(size_modes(&rect, false, true), vec![Px, Percent]);
        assert_eq!(size_modes(&rect, false, false), vec![Px]);
        assert_eq!(
            size_modes(&path, false, true),
            vec![Px],
            "the layout cannot resize a path, so a percentage would do nothing"
        );
    }

    /// Where a picture's wall lands — the midpoint of a start alignment's edge
    /// line, which says which side it is on and nothing about which way the line
    /// was drawn (a first endpoint would, and a quarter turn reverses it on a wall
    /// that is on the right side all the same).
    fn wall(g: Glyph, flex: Flex) -> (f32, f32) {
        let o = Orient::of(g, &flex);
        marks(g)
            .into_iter()
            .find_map(|m| match o.apply(m) {
                Mark::Edge { x1, y1, x2, y2, .. } => Some(((x1 + x2) / 2.0, (y1 + y2) / 2.0)),
                Mark::Bar { .. } => None,
            })
            .expect("a start alignment has a wall")
    }

    /// **The pictures turn with the layout through the diagonal, not by a
    /// rotation** (`Orient`): in a column, `justify-content: flex-start`'s wall is
    /// on top and `align-items: flex-start`'s on the left, the two starts a column
    /// has. `row-reverse` puts the justify wall on the right, and `wrap-reverse`
    /// the align wall at the bottom — CSS swapping cross-start and cross-end.
    ///
    /// **Flip run**, the transpose replaced with a quarter turn clockwise
    /// (`(x, y) → (16 − y, x)`): fails on *"align-start in a column"*, the wall
    /// at x 14.5 — on the right — the predicted site.
    #[test]
    fn the_pictures_turn_with_the_layout_through_the_diagonal() {
        let column = Flex {
            direction: FlexDirection::Column,
            ..Default::default()
        };
        let start = Glyph::Justify(JustifyContent::Start);
        let align = Glyph::Align(AlignItems::Start);
        assert_eq!(
            wall(start, Flex::default()),
            (1.5, 8.0),
            "justify in a row: left"
        );
        assert_eq!(
            wall(align, Flex::default()),
            (8.0, 1.5),
            "align in a row: top"
        );
        assert_eq!(wall(start, column), (8.0, 1.5), "justify-start in a column");
        assert_eq!(wall(align, column), (1.5, 8.0), "align-start in a column");
        let reversed = Flex {
            direction: FlexDirection::RowReverse,
            ..Default::default()
        };
        assert_eq!(wall(start, reversed), (14.5, 8.0), "row-reverse: right");
        let lines_reversed = Flex {
            wrap: FlexWrap::WrapReverse,
            ..Default::default()
        };
        assert_eq!(
            wall(align, lines_reversed),
            (8.0, 14.5),
            "wrap-reverse: align-start at the bottom"
        );
    }

    /// **A resize's receipt lasts exactly as long as its step is the top of the
    /// history** (`session::FlexReceipt`, §15 D880). An item property set by hand
    /// is no receipt; a resize that stops growth is one, naming what it flipped;
    /// an undo takes it away and a redo, putting the step back on top, brings it
    /// back; the next commit ends it.
    ///
    /// **Flip run**, the undo-depth half of `flex_receipt`'s test dropped: fails on
    /// *"an undo takes it away"* — undo does not move the session's revision, so the
    /// revision alone cannot see it — the predicted site.
    #[test]
    fn a_resize_leaves_a_receipt_until_anything_else_is_committed() {
        let mut s = scene();
        set_item(&mut s.app, s.a, |i| i.grow = 1.0);
        assert!(
            s.app.session.flex_receipt().is_none(),
            "an item property set by hand is not a receipt"
        );
        let tall = drawn(&s.app, s.a).height();
        s.app
            .session
            .commit(Transaction(vec![Operation::SetGeometry {
                id: s.a,
                geometry: GeometryPatch::Size(Size::new(100.0, tall)),
            }]));
        let receipt = s
            .app
            .session
            .flex_receipt()
            .expect("a resize that stops growth");
        assert_eq!(receipt.held.len(), 1);
        let (id, was, now) = receipt.held[0];
        assert_eq!(id, s.a);
        assert_eq!((was.grow, was.shrink), (1.0, 1.0));
        assert_eq!(
            (now.grow, now.shrink),
            (0.0, 0.0),
            "main axis: growth stopped"
        );
        assert_eq!(now.align_self, None, "the cross axis was not resized");

        s.app.session.undo();
        assert!(
            s.app.session.flex_receipt().is_none(),
            "an undo takes it away"
        );
        s.app.session.redo();
        assert!(
            s.app.session.flex_receipt().is_some(),
            "a redo puts the step back on top, and the receipt with it"
        );
        s.app
            .session
            .commit(Transaction(vec![Operation::SetGeometry {
                id: s.frame,
                geometry: GeometryPatch::Size(Size::new(500.0, 200.0)),
            }]));
        assert!(
            s.app.session.flex_receipt().is_none(),
            "the next commit ends it"
        );
    }

    /// **`display: none` keeps every child where it is drawn**, and a frame that
    /// hugged its children the size it hugged to (`OndinApp::set_display`, §15
    /// D878) — the rects are stored at (300, 150) and laid at the row's slots.
    ///
    /// **Flip run**, the children's `baked_ops` dropped: fails on *"a stayed where
    /// it was drawn"*, the rect back at its stored x 300 — the predicted site.
    #[test]
    fn display_none_keeps_every_child_where_it_is_drawn() {
        let mut s = scene();
        set_item(&mut s.app, s.frame, |i| i.width = Dimension::FitContent);
        let (f0, a0, b0) = (
            drawn(&s.app, s.frame),
            drawn(&s.app, s.a),
            drawn(&s.app, s.b),
        );
        assert_eq!(
            f0.width(),
            150.0,
            "the fixture hugs: 20 + 40 + 10 + 60 + 20"
        );
        assert_eq!(a0.x0, 20.0, "and lays out");

        s.app.set_display(&[s.frame], 0);
        assert!(s.app.session.doc.get(s.frame).unwrap().display().is_none());
        assert_eq!(drawn(&s.app, s.a), a0, "a stayed where it was drawn");
        assert_eq!(drawn(&s.app, s.b), b0, "and b");
        assert_eq!(
            drawn(&s.app, s.frame).width(),
            150.0,
            "the frame kept the size it hugged to"
        );
        assert_eq!(
            s.app.session.doc.get(s.frame).unwrap().item().width,
            Dimension::Auto
        );
        s.app.session.undo();
        assert!(
            s.app.session.doc.get(s.frame).unwrap().display().is_some(),
            "one undo step"
        );
    }

    /// **A mode picked from the W/H menu moves nothing** (`size_mode_tx`, §15
    /// D879): a hugging frame switched to px keeps the width it hugged to, now as
    /// its stored size; an item switched to % gets the percentage of the content
    /// box it already fills, 40 of 110 (the hugging frame's 150 less 40 of padding).
    #[test]
    fn a_mode_picked_from_the_size_menu_moves_nothing() {
        let mut s = scene();
        set_item(&mut s.app, s.frame, |i| i.width = Dimension::FitContent);
        let size = drawn(&s.app, s.frame).size();
        let tx = s.app.size_mode_tx(s.frame, true, SizeMode::Px, size);
        s.app.session.commit(tx);
        let frame = s.app.session.doc.get(s.frame).unwrap();
        assert_eq!(frame.item().width, Dimension::Auto);
        assert_eq!(
            frame.kind(),
            &NodeKind::Artboard {
                size: Size::new(150.0, 200.0)
            },
            "the hugged width, now the frame's own"
        );

        let tx = s
            .app
            .size_mode_tx(s.a, true, SizeMode::Percent, drawn(&s.app, s.a).size());
        s.app.session.commit(tx);
        assert_eq!(
            s.app.session.doc.get(s.a).unwrap().item().width,
            Dimension::Percent(36.4)
        );
        assert!(
            (drawn(&s.app, s.a).width() - 40.0).abs() < 0.1,
            "drawn where it was, to the tenth of a percent: {}",
            drawn(&s.app, s.a).width()
        );
    }

    /// **The out-of-flow block's buttons bring a layer back**, at its own place
    /// among its siblings (`back_into_flow`): a rect pinned to the frame's
    /// bottom-right corner leaves the row, which closes up; *Unpin insets* puts it
    /// back first in the row. A hidden one is shown.
    #[test]
    fn the_way_back_into_the_flow_puts_a_layer_at_its_own_place() {
        let mut s = scene();
        s.app.session.commit(Transaction(vec![Operation::SetInsets {
            id: s.a,
            insets: Insets {
                right: Some(LengthPct::Px(0.0)),
                bottom: Some(LengthPct::Px(0.0)),
                ..Default::default()
            },
        }]));
        assert_eq!(s.app.out_of_flow(s.a), Some(OutOfFlow::Absolute));
        assert_eq!(drawn(&s.app, s.b).x0, 20.0, "the row closed up");
        assert_eq!(
            pinned_edges(s.app.session.doc.get(s.a).unwrap().insets()),
            "Pinned to its container's right and bottom edges"
        );

        s.app.back_into_flow(&[s.a], OutOfFlow::Absolute);
        assert_eq!(s.app.out_of_flow(s.a), None);
        assert!(s.app.session.doc.get(s.a).unwrap().insets().is_unset());
        assert_eq!(drawn(&s.app, s.a).x0, 20.0, "first in the row again");
        assert_eq!(drawn(&s.app, s.b).x0, 70.0, "and b after it");

        s.app
            .session
            .commit(Transaction(vec![Operation::SetVisible {
                id: s.b,
                visible: false,
            }]));
        assert_eq!(s.app.out_of_flow(s.b), Some(OutOfFlow::Hidden));
        s.app.back_into_flow(&[s.b], OutOfFlow::Hidden);
        assert!(s.app.session.doc.get(s.b).unwrap().visible());
    }

    /// One card drawn headlessly in the inspector column's 284, with real pointer
    /// events — the harness the field-scrub tests share (§15 D885, D901). `card`
    /// is which card: `inspector_container` or `inspector_item`.
    struct Panel {
        ctx: egui::Context,
        app: OndinApp,
        time: f64,
        card: fn(&mut OndinApp, &mut egui::Ui),
    }

    impl Panel {
        fn new(app: OndinApp, card: fn(&mut OndinApp, &mut egui::Ui)) -> Self {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            Panel {
                ctx,
                app,
                time: 0.0,
                card,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
            self.time += 0.1;
            let (app, card) = (&mut self.app, self.card);
            self.ctx.run_ui(
                egui::RawInput {
                    time: Some(self.time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_max_width(284.0);
                    card(app, ui);
                },
            )
        }

        /// Settled, then where the first run reading exactly `text` is.
        fn run(&mut self, text: &str) -> egui::Pos2 {
            let mut out = self.frame(Vec::new());
            for _ in 0..2 {
                out = self.frame(Vec::new());
            }
            out.shapes
                .iter()
                .find_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text() == text => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no run reads {text:?}"))
        }

        /// Press at `from`, drag 80 points right in four steps, let go.
        fn scrub(&mut self, from: egui::Pos2) {
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            self.frame(vec![egui::Event::PointerMoved(from), button(from, true)]);
            for step in 1..=4 {
                let at = from + egui::vec2(20.0 * step as f32, 0.0);
                self.frame(vec![egui::Event::PointerMoved(at)]);
            }
            let at = from + egui::vec2(80.0, 0.0);
            self.frame(vec![button(at, false)]);
            for _ in 0..3 {
                self.frame(Vec::new());
            }
        }
    }

    /// **A scrub on a Container field lands, as one undo step** (§15 D885) — the
    /// Column gap, then the left-and-right padding, each pressed on its digits,
    /// dragged 80 points and let go, through the real card and the real valve.
    ///
    /// Every number field in these cards used to follow the drag and snap back on
    /// release: it built its transaction only on a frame where the number moved,
    /// and the release frame is the one frame the valve commits on and the one
    /// frame nothing moves. No test drove a field before this one; the cards'
    /// tests called the methods the fields call.
    ///
    /// **Flip run**, `edited` answering `moved` alone (the latch dropped, which is
    /// the code as it was): fails on *"the gap landed"* with the gap back at 10 —
    /// the predicted site.
    #[test]
    fn a_scrub_on_a_container_field_lands_as_one_undo_step() {
        let s = scene();
        let frame_id = s.frame;
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        let flex = |app: &OndinApp| match app.session.doc.get(frame_id).unwrap().display() {
            Some(Display::Flex(f)) => *f,
            _ => panic!("the fixture lost its flex layout"),
        };

        assert_eq!(flex(&p.app).column_gap, 10.0, "the fixture");
        let depth = p.app.session.history.undo_depth();
        // The gap's digits: the only run reading 10 (the padding reads 20).
        let at = p.run("10");
        p.scrub(at);
        let gap = flex(&p.app).column_gap;
        assert!(gap > 15.0, "the gap landed: {gap}, from 10 by 80 points");
        assert_eq!(p.app.session.history.undo_depth(), depth + 1, "as one step");

        // The left-and-right padding: the first run reading 20, its row being
        // above top-and-bottom's.
        let at = p.run("20");
        p.scrub(at);
        let padding = flex(&p.app).padding;
        assert!(
            padding[1] > 25.0 && padding[1] == padding[3],
            "left and right landed together: {padding:?}"
        );
        assert_eq!(padding[0], 20.0, "and top was left alone");
    }

    /// **The Item card's fields land too** (§15 D885's other half, never driven
    /// until §15 D901): `a`'s Flex grow scrubbed from 0, then its basis scrubbed
    /// from the `auto` in its digits — each pressed on its digits, dragged 80
    /// points and let go, one undo step apiece. The basis starts from the drawn
    /// width under the word (§15 D895's `under`), so it lands in px above 40.
    ///
    /// **Flip run**, `edited` answering `moved` alone: fails on *"the grow
    /// landed"*, grow back at 0 — the predicted site.
    #[test]
    fn a_scrub_on_an_item_field_lands_as_one_undo_step() {
        let s = scene();
        let a = s.a;
        let mut p = Panel::new(s.app, OndinApp::inspector_item);
        p.app.session.selection.set(vec![a]);
        let item = |app: &OndinApp| *app.session.doc.get(a).unwrap().item();
        assert_eq!(item(&p.app).grow, 0.0, "the fixture");
        let depth = p.app.session.history.undo_depth();

        let at = p.run("0");
        p.scrub(at);
        let grow = item(&p.app).grow;
        assert!(grow > 0.5, "the grow landed: {grow}");
        assert_eq!(p.app.session.history.undo_depth(), depth + 1, "as one step");

        let at = p.run("auto");
        p.scrub(at);
        match item(&p.app).basis {
            Dimension::Px(v) => assert!(v > 40.0, "the basis landed in px: {v}"),
            other => panic!("the basis did not land: {other:?}"),
        }
        assert_eq!(p.app.session.history.undo_depth(), depth + 2, "one more");
    }

    /// The Transform card as the single-layer inspector draws it — its world and
    /// size read through the preview, `inspector_single`'s way — over the first
    /// selected layer, a rect.
    fn transform_card(app: &mut OndinApp, ui: &mut egui::Ui) {
        let id = app.session.selection.ids()[0];
        let world = app
            .session
            .preview_world_transform(id)
            .unwrap_or_default()
            .as_coeffs();
        let size = app.session.display_node(id).and_then(|n| match n.kind() {
            NodeKind::Rect { size, .. } => Some(*size),
            _ => None,
        });
        app.inspector_transform(ui, id, world, size);
    }

    /// **A stretched item's H scrubs, and the preview follows it** (§15 D904) —
    /// `a`, stretched to the row's 160, its H pressed on its digits and dragged 80
    /// points, through the real card and the real valve. It lands taller, out of
    /// the stretch, as one step.
    ///
    /// The field reads its size back through the preview, which re-lays the row on
    /// every frame of the scrub; with the item still `align-self: stretch` there,
    /// the preview put it straight back to 160, so each frame's scrub started from
    /// 160 again and the release committed 160. Reported as "it doesn't let me
    /// update the height in Transform".
    ///
    /// **Flip run**, `set_preview` without the holds: fails on *"the height
    /// landed"* at 160 — the predicted site.
    #[test]
    fn a_stretched_items_height_scrubs_in_the_transform_card() {
        let s = scene();
        let a = s.a;
        let mut p = Panel::new(s.app, transform_card);
        p.app.session.selection.set(vec![a]);
        assert_eq!(drawn(&p.app, a).height(), 160.0, "the fixture stretches");
        let depth = p.app.session.history.undo_depth();

        let at = p.run("160");
        p.scrub(at);
        let h = drawn(&p.app, a).height();
        assert!(h > 170.0, "the height landed: {h}, from 160 by 80 points");
        assert_eq!(
            p.app.session.doc.get(a).unwrap().item().align_self,
            Some(AlignItems::Start),
            "out of the stretch"
        );
        assert_eq!(p.app.session.history.undo_depth(), depth + 1, "as one step");
    }

    /// **The sizing menu opens under the unit that was clicked** (§15 D907) — W's
    /// `px`, at the right-hand end of the field, clicked; the menu's `%` row is
    /// found beneath it, its text within a few points of the unit's. It opened
    /// under the number's response, which starts at the prefix, so the menu hung
    /// under the "W" at the field's far end.
    ///
    /// **Flip runs**: the popup anchored on the number's response with its default
    /// alignment, as it was, fails on *"under the unit"*, the row's text centred at
    /// x 46 against the unit's 126 — the predicted site. The row padding dropped
    /// fails on *"room either side"* at 2 and 2.2 points, `Popup::menu`'s own style —
    /// which the first cut of this assertion did **not** catch: searched from the
    /// front, it measured against the rotation field's ground under the popup.
    #[test]
    fn the_size_menu_opens_under_its_unit() {
        let s = scene();
        let a = s.a;
        let mut p = Panel::new(s.app, transform_card);
        p.app.session.selection.set(vec![a]);
        let unit = p.run("px");
        let button = |pressed| egui::Event::PointerButton {
            pos: unit,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        p.frame(vec![egui::Event::PointerMoved(unit), button(true)]);
        p.frame(vec![button(false)]);
        let row = p.run("%");
        assert!(row.y > unit.y, "below it: {row:?} against {unit:?}");
        assert!(
            (row.x - unit.x).abs() < 24.0,
            "under the unit: the row at x {} against the unit's {}",
            row.x,
            unit.x
        );

        // The lit row — `px`, the mode W is in — and the highlight behind its text.
        // Searched from the end: the popup paints last, and the fields under it have
        // grounds that contain the same point.
        let out = p.frame(Vec::new());
        let text = out
            .shapes
            .iter()
            .rev()
            .find_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == "px" && t.pos.y > unit.y => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                _ => None,
            })
            .expect("the menu's px row");
        let lit = out
            .shapes
            .iter()
            .rev()
            .find_map(|cs| match &cs.shape {
                egui::epaint::Shape::Rect(r)
                    if r.fill.a() > 0
                        && r.rect.contains(text.center())
                        && r.rect.height() < 40.0 =>
                {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("the lit row's highlight");
        let (left, right) = (text.left() - lit.left(), lit.right() - text.right());
        assert!(
            left >= 7.0 && right >= 7.0,
            "room either side of the text: {left} and {right}"
        );
    }

    /// **A stretched item dragged by its top handle previews as it lands** —
    /// shorter, its bottom held at the row's (§15 D904, D905). The preview used to
    /// stretch it straight back until the release; and the release, aligning it to
    /// the start, moved it to the top of the row — away from the edge the user held.
    ///
    /// **Flip runs**, each failing on *"the preview follows the handle"*:
    /// `set_preview` without the holds, the preview still 20..180;
    /// `resized_from_cross_start` answering `false`, at 20..90 — the holds are in
    /// the preview and aligned to the start, which is the jump the release made;
    /// and `RenderOverrides`' `flex_relayout` falling back to the document's
    /// transform where `item_placed` answers `local: None`, at 150..220 — the rect's
    /// stored y, since the end-aligned slot is exactly the tool's shifted origin.
    #[test]
    fn a_stretched_item_dragged_by_its_top_previews_as_it_lands() {
        let mut s = scene();
        let tx = crate::tools::resize_layer(
            &s.app.session.doc,
            &s.app.session.resolved,
            s.a,
            crate::preview::Handle::Top,
            ondin_core::kurbo::Point::new(40.0, 110.0),
            crate::tools::Resize::default(),
        );
        s.app.session.set_preview(&tx);
        let seen = s.app.session.preview_world_bounds(s.a).expect("measured");
        assert_eq!(
            (seen.y0.round(), seen.y1.round()),
            (110.0, 180.0),
            "the preview follows the handle"
        );
        s.app.session.commit(tx);
        let landed = drawn(&s.app, s.a);
        assert_eq!(
            (landed.y0.round(), landed.y1.round()),
            (110.0, 180.0),
            "and lands there"
        );
        assert_eq!(
            s.app.session.doc.get(s.a).unwrap().item().align_self,
            Some(AlignItems::End)
        );
    }

    /// **A field over a selection that disagrees reads "Mixed", and a basis at
    /// `auto` reads `auto`** — the maintainer's rulings of §15 D892 and D895,
    /// asserted on the Item card's painted text for two rects that disagree about
    /// `flex-grow` and agree on an `auto` basis.
    ///
    /// **Flip runs**: `mixed_if` back to a dash fails on *"the grow field says
    /// so"*, "Mixed" counted 0 against 1, predicted. The keyword word dropped from
    /// `size_field` (the basis back on the number path) fails on **the same
    /// line**, 2 against 1 — not on *"the basis reads its keyword"* as predicted:
    /// with no number and no word the basis's digits read "Mixed" too, which is
    /// exactly the confusion D892 would have introduced without D895. The unit
    /// handed back to the keyword face (§15 D906's `word.is_none()` gate dropped)
    /// fails on *"and has no unit"*, 1 against 0, predicted.
    #[test]
    fn a_mixed_field_reads_mixed_and_an_unset_basis_reads_auto() {
        let mut s = scene();
        set_item(&mut s.app, s.a, |i| i.grow = 1.0);
        s.app.session.selection.set(vec![s.a, s.b]);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(284.0);
            s.app.inspector_item(ui);
        });
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        let count = |w: &str| texts.iter().filter(|t| *t == w).count();
        assert_eq!(count("Mixed"), 1, "the grow field says so: {texts:?}");
        assert_eq!(count("auto"), 1, "the basis reads its keyword: {texts:?}");
        assert_eq!(count("–"), 0, "and has no unit (§15 D906): {texts:?}");
    }

    /// **A segment row over containers that disagree marks the mixed state**
    /// (§15 D892): two frames, a row and a column, selected together — the
    /// direction row raises nothing and draws `ui::segment_mixed`'s dash where its
    /// first arrow was, the Type panel's convention. It drew nothing at all, which
    /// `segment_mixed`'s own doc says reads as a bug. The display and wrap rows
    /// agree and draw no dash, and no Container field has a unit, so one dash is
    /// the whole count.
    ///
    /// **Flip run**, the direction row's `None if i == 0` arm deleted: fails on
    /// *"the direction row says it is mixed"*, 0 against 1, predicted.
    #[test]
    fn a_segment_row_over_containers_that_disagree_marks_it() {
        let mut s = scene();
        let mut ids = IdSource::new(0xCB);
        let column = ids.mint();
        s.app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: column,
                parent: s.app.session.doc.root(),
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(200.0, 200.0),
                },
                transform: Some(Affine::translate((600.0, 0.0))),
                name: None,
            },
            Operation::SetDisplay {
                id: column,
                display: Some(Display::Flex(Flex {
                    direction: FlexDirection::Column,
                    ..Default::default()
                })),
            },
        ]));
        s.app.session.selection.set(vec![s.frame, column]);
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_max_width(284.0);
            s.app.inspector_container(ui);
        });
        let dashes = out
            .shapes
            .iter()
            .filter(
                |cs| matches!(&cs.shape, egui::epaint::Shape::Text(t) if t.galley.text() == "–"),
            )
            .count();
        assert_eq!(dashes, 1, "the direction row says it is mixed");
    }

    /// **Each card offers itself for what it describes and draws without
    /// panicking** — Container for the frame, Item for the rects, both for a
    /// selection of the two, the out-of-flow block for a pinned rect, and the
    /// display row alone for a frame with no layout.
    #[test]
    fn each_card_offers_itself_for_what_it_describes() {
        let mut s = scene();
        s.app.session.selection.set(vec![s.frame]);
        assert_eq!(s.app.container_subjects(), vec![s.frame]);
        assert!(
            s.app.item_subjects().is_empty(),
            "a top-level frame is in no layout"
        );
        s.app.session.selection.set(vec![s.a, s.b]);
        assert!(
            s.app.container_subjects().is_empty(),
            "a rect is no container"
        );
        assert_eq!(s.app.item_subjects(), vec![s.a, s.b]);

        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let draw = |app: &mut OndinApp, selection: Vec<NodeId>| {
            app.session.selection.set(selection);
            // The inspector column's own width (`OndinApp::inspector_column`): the
            // gap fields carry their CSS name as a unit, and at 240 — the Position
            // card's test width — a half-width one has no room left for its digits.
            let _ = ctx.run_ui(Default::default(), |ui| {
                ui.set_max_width(284.0);
                app.inspector_item(ui);
                app.inspector_container(ui);
                let id = app.session.selection.ids()[0];
                let world = app
                    .session
                    .resolved
                    .world_transform(id)
                    .unwrap_or_default()
                    .as_coeffs();
                let size = Some(drawn(app, id).size());
                app.inspector_transform(ui, id, world, size);
            });
        };
        draw(&mut s.app, vec![s.frame]);
        draw(&mut s.app, vec![s.a]);
        draw(&mut s.app, vec![s.frame, s.a]);
        // The limits' disclosure open, and padding that differs side to side —
        // which opens the four side fields by itself.
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("flex-item-limits"), true));
        let tx = s
            .app
            .flex_tx(&[s.frame], |f| f.padding = [4.0, 8.0, 12.0, 16.0]);
        s.app.session.commit(tx);
        draw(&mut s.app, vec![s.frame, s.a]);
        set_item(&mut s.app, s.a, |i| i.grow = 1.0);
        draw(&mut s.app, vec![s.a, s.b]);
        s.app.session.commit(Transaction(vec![Operation::SetInsets {
            id: s.a,
            insets: Insets {
                top: Some(LengthPct::Px(0.0)),
                ..Default::default()
            },
        }]));
        draw(&mut s.app, vec![s.a]);
        s.app.set_display(&[s.frame], 0);
        draw(&mut s.app, vec![s.frame]);
    }

    // --- grid's rows (§15 D920) -------------------------------------------------

    /// [`scene`] with the frame's row turned into a grid of `100px 1fr` columns —
    /// its gap 10 and padding 20 kept, as picking *Grid* keeps them.
    fn grid_scene() -> Scene {
        let mut s = scene();
        s.app.set_display(&[s.frame], 2);
        let Some(Display::Grid(g)) = s.app.session.doc.get(s.frame).unwrap().display().cloned()
        else {
            panic!("the fixture's grid");
        };
        let columns = ondin_core::container::parse_tracks("100px 1fr").unwrap();
        s.app
            .session
            .commit(Transaction(vec![Operation::SetDisplay {
                id: s.frame,
                display: Some(Display::Grid(Grid { columns, ..g })),
            }]));
        s
    }

    fn press(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    impl Panel {
        /// A click at `at`, and a frame to let it land.
        fn click(&mut self, at: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(at), press(at, true)]);
            self.frame(vec![press(at, false)]);
            self.frame(Vec::new());
        }

        /// Focus the field whose text reads `shown`, type `text` over it (a field
        /// selects all on focus) and leave by `leave`.
        fn retype(&mut self, shown: &str, text: &str, leave: egui::Key) {
            let at = self.run(shown);
            self.click(at);
            self.frame(vec![egui::Event::Text(text.into())]);
            self.frame(vec![key(leave)]);
            for _ in 0..2 {
                self.frame(Vec::new());
            }
        }

        /// Settled, then where every run reading exactly `text` is, top to bottom.
        fn runs(&mut self, text: &str) -> Vec<egui::Pos2> {
            self.shapes()
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text() == text => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .collect()
        }

        /// Settled, then what the card painted.
        fn shapes(&mut self) -> Vec<egui::epaint::ClippedShape> {
            for _ in 0..2 {
                self.frame(Vec::new());
            }
            self.frame(Vec::new()).shapes
        }

        /// Press at `from`, move to `to` in four steps, and let go only if
        /// `release` — a grip's drag.
        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2, release: bool) {
            self.frame(vec![egui::Event::PointerMoved(from), press(from, true)]);
            for step in 1..=4 {
                let at = from + (to - from) * (step as f32 / 4.0);
                self.frame(vec![egui::Event::PointerMoved(at)]);
            }
            if release {
                self.frame(vec![press(to, false)]);
                self.frame(Vec::new());
            }
        }
    }

    /// Whether `shapes` hold the grip reorder's insertion line — the one
    /// horizontal `ACCENT` stroke 1.5 wide the track list draws.
    fn reorder_line(shapes: &[egui::epaint::ClippedShape]) -> bool {
        shapes.iter().any(|cs| {
            matches!(
                &cs.shape,
                egui::epaint::Shape::LineSegment { points: [a, b], stroke }
                    if a.y == b.y && stroke.color == color::ACCENT && stroke.width == 1.5
            )
        })
    }

    fn grid_of(app: &OndinApp, id: NodeId) -> Grid {
        match app.session.doc.get(id).unwrap().display() {
            Some(Display::Grid(g)) => g.clone(),
            other => panic!("not a grid: {other:?}"),
        }
    }

    /// **Picking *Grid* lays the frame out as a grid and keeps what the two share**
    /// (§15 D920): the padding and the gaps travel from the flex row, and nothing
    /// else — the tracks start empty, CSS's `none`. Driven through the display
    /// row's cell, which was disabled until the grid step.
    ///
    /// **Flip run**, `set_display`'s `kept` returning the fresh layout untouched:
    /// fails on *"the padding travelled"* at 0, the predicted site.
    #[test]
    fn picking_grid_keeps_the_padding_and_the_gaps() {
        let s = scene();
        let frame_id = s.frame;
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        let at = p.run("Grid");
        p.click(at);
        let g = grid_of(&p.app, frame_id);
        assert_eq!(g.padding, [20.0; 4], "the padding travelled");
        assert_eq!(g.column_gap, 10.0, "and the gap");
        assert!(g.columns.is_empty() && g.rows.is_empty(), "no tracks yet");
    }

    /// **The column list's `+` adds a `1fr` track, the cross removes one, and its
    /// count says how many tracks the list makes** — `100px 1fr` reads *2 tracks*,
    /// grows to `100px 1fr 1fr`, and loses its first to `1fr 1fr`.
    #[test]
    fn a_track_is_added_with_the_plus_and_removed_with_the_cross() {
        let s = grid_scene();
        let frame_id = s.frame;
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        p.run("2 tracks");
        // The first `+` is the columns' — their head is above the rows'.
        let at = p.run(icon::PLUS);
        p.click(at);
        let css =
            |app: &OndinApp| ondin_core::container::tracks_css(&grid_of(app, frame_id).columns);
        assert_eq!(css(&p.app), "100px 1fr 1fr");
        let at = p.run(icon::X);
        p.click(at);
        assert_eq!(css(&p.app), "1fr 1fr", "the first entry's cross");
    }

    /// **The CSS line applies on Enter and never on Escape** (`ui::defocus_commits`,
    /// §15 D841): `100px 1fr` retyped `200px repeat(2, 1fr)` and Enter lands as the
    /// list; retyped `50px` and Escape leaves it; retyped `100` — no unit — is
    /// refused with its reason under the field, and the list stays.
    ///
    /// **Flip run**, `css_field` committing on a bare `lost_focus()`: fails on
    /// *"Escape abandons it"* with `50px` landed, the predicted site.
    #[test]
    fn the_css_line_applies_on_enter_and_never_on_escape() {
        let s = grid_scene();
        let frame_id = s.frame;
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        let css =
            |app: &OndinApp| ondin_core::container::tracks_css(&grid_of(app, frame_id).columns);
        p.retype("100px 1fr", "200px repeat(2, 1fr)", egui::Key::Enter);
        assert_eq!(css(&p.app), "200px repeat(2, 1fr)", "Enter applies it");
        p.retype("200px repeat(2, 1fr)", "50px", egui::Key::Escape);
        assert_eq!(css(&p.app), "200px repeat(2, 1fr)", "Escape abandons it");
        let depth = p.app.session.history.undo_depth();
        p.retype("200px repeat(2, 1fr)", "100", egui::Key::Enter);
        assert_eq!(css(&p.app), "200px repeat(2, 1fr)", "a bad list is refused");
        assert_eq!(p.app.session.history.undo_depth(), depth, "with no step");
        p.run("100 needs a unit: px, % or fr");
    }

    /// **A grid item's card has its lines and self-alignments where a flex item's
    /// has grow, shrink and basis** (§15 D920), and **a line is typed as CSS**:
    /// `a`'s column start retyped `2` and its end `span 2` land as
    /// `2 / span 2`.
    #[test]
    fn a_grid_items_lines_are_typed_as_css() {
        let s = grid_scene();
        let a = s.a;
        let mut p = Panel::new(s.app, OndinApp::inspector_item);
        p.app.session.selection.set(vec![a]);
        p.run("Grid column");
        // A glyph combo's label and face are one galley; `auto` reads what the
        // container's items row resolves to, `normal` (§15 D915).
        p.run("Justify selfAuto · Normal");
        let out = p.frame(Vec::new());
        assert!(
            !out.shapes.iter().any(|cs| matches!(
                &cs.shape,
                egui::epaint::Shape::Text(t) if t.galley.text() == "Flex grow"
            )),
            "no flex rows in a grid"
        );
        // Four fields read `auto`; the first is the column's start.
        p.retype("auto", "2", egui::Key::Enter);
        let start = p.app.session.doc.get(a).unwrap().item().grid_column.start;
        assert_eq!(start, ondin_core::container::GridPlacement::Line(2));
        // The column's end is now the first `auto` left.
        p.retype("auto", "span 2", egui::Key::Enter);
        let lines = p.app.session.doc.get(a).unwrap().item().grid_column;
        assert_eq!(
            lines.end,
            ondin_core::container::GridPlacement::Span(2),
            "{lines:?}"
        );
    }

    /// The column list of `id`'s grid, as its CSS line reads it.
    fn columns_css(app: &OndinApp, id: NodeId) -> String {
        ondin_core::container::tracks_css(&grid_of(app, id).columns)
    }

    /// **A track row's grip drops its entry in the slot it is let go over, as one
    /// undo step** (§15 D920, which pinned `reordered`'s arithmetic alone): the
    /// first of `100px 1fr`'s grips dragged below the second row's middle lands
    /// it last, `1fr 100px`, and the insertion line is drawn while the button is
    /// held and gone once it is let go.
    ///
    /// **Flip run**, the list's `released |= grip.drag_stopped()` dropped — the
    /// release no longer reaching the commit: fails on *"dropped last"* with the
    /// list as it was, the predicted site.
    #[test]
    fn a_track_rows_grip_drag_reorders_the_list() {
        let s = grid_scene();
        let frame_id = s.frame;
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        let grips = p.runs(icon::DOTS_SIX_VERTICAL);
        assert_eq!(grips.len(), 2, "the fixture: a grip per column entry");
        let depth = p.app.session.history.undo_depth();
        let below = grips[1] + egui::vec2(0.0, 8.0);
        p.drag(grips[0], below, false);
        assert!(reorder_line(&p.frame(Vec::new()).shapes), "the line, held");
        p.frame(vec![press(below, false)]);
        p.frame(Vec::new());
        assert_eq!(columns_css(&p.app, frame_id), "1fr 100px", "dropped last");
        assert_eq!(p.app.session.history.undo_depth(), depth + 1, "one step");
        assert!(!reorder_line(&p.shapes()), "and gone with the drag");
    }

    /// **A grip drag let go where no grip saw it is over, and draws nothing
    /// later** (§15 D920's amendment, read and not tested until now): a drag
    /// begun on a grip, the selection changed to an item with the button still
    /// down and let go there, and the frame selected again — then a press on
    /// nothing at all draws no insertion line and reorders nothing.
    ///
    /// **Flip run**, the clean-up's `remove::<TrackDrag>` dropped: fails on *"no
    /// line follows a later press"*, the stale drag's line drawn under the new
    /// press — the predicted site, and the symptom `arch-scribe` read.
    #[test]
    fn a_grip_drag_let_go_elsewhere_draws_nothing_later() {
        let s = grid_scene();
        let (frame_id, a) = (s.frame, s.a);
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        let grips = p.runs(icon::DOTS_SIX_VERTICAL);
        let away = grips[0] + egui::vec2(0.0, 40.0);
        p.drag(grips[0], away, false);
        assert!(
            reorder_line(&p.frame(Vec::new()).shapes),
            "the fixture: held"
        );
        p.app.session.selection.set(vec![a]);
        p.frame(vec![press(away, false)]);
        p.frame(Vec::new());
        p.app.session.selection.set(vec![frame_id]);
        p.frame(Vec::new());
        let nowhere = egui::pos2(1000.0, 800.0);
        p.drag(nowhere, nowhere + egui::vec2(0.0, 30.0), false);
        assert!(
            !reorder_line(&p.frame(Vec::new()).shapes),
            "no line follows a later press"
        );
        p.frame(vec![press(nowhere, false)]);
        p.frame(Vec::new());
        assert_eq!(columns_css(&p.app, frame_id), "100px 1fr", "nothing moved");
    }

    /// **A refusal stays under the CSS line only while the line reads what was
    /// refused, and only until the field is engaged again** (§15 D920's
    /// amendment): `100` refused, then a `+` changes the list and the reason
    /// goes; refused again, then the field clicked into, and it goes too.
    ///
    /// **Flip runs**: `refusal` answering whatever was stored, the `about ==
    /// shown` test dropped, fails on *"a `+` makes it about a list that is gone"*;
    /// `css_field`'s removal on `gained_focus` dropped fails on *"gone once the
    /// field is engaged again"* — each the predicted site.
    #[test]
    fn a_refusal_lasts_until_the_list_or_the_field_moves_on() {
        const WHY: &str = "100 needs a unit: px, % or fr";
        let s = grid_scene();
        let frame_id = s.frame;
        let mut p = Panel::new(s.app, OndinApp::inspector_container);
        p.app.session.selection.set(vec![frame_id]);
        p.retype("100px 1fr", "100", egui::Key::Enter);
        assert_eq!(p.runs(WHY).len(), 1, "the fixture: refused, and why");
        let at = p.run(icon::PLUS);
        p.click(at);
        assert_eq!(columns_css(&p.app, frame_id), "100px 1fr 1fr");
        assert!(
            p.runs(WHY).is_empty(),
            "a `+` makes it about a list that is gone"
        );
        p.retype("100px 1fr 1fr", "100", egui::Key::Enter);
        assert_eq!(p.runs(WHY).len(), 1, "refused again");
        let at = p.run("100px 1fr 1fr");
        p.click(at);
        assert!(
            p.runs(WHY).is_empty(),
            "gone once the field is engaged again"
        );
    }

    /// **A grid item's resize receipt outlines the row it names** (§15 D922) — `a`
    /// set to `justify-self: stretch` by hand, then resized narrower: the
    /// commit holds it with `start`, the receipt says *"justify self"*, and the
    /// Item card strokes the Justify self row in the accent and not the Align
    /// self row. Until D922 there was **no receipt at all** — `growth_held` read
    /// grow, shrink and `align-self` and not `justify-self`, the one grid holds a
    /// width with — and the row was not outlined had there been one; flex's
    /// align-self row always was (§15 D880).
    ///
    /// **Flip runs**: `grid_item_rows` handed `[false, false]` fails on *"the
    /// justify row outlined"*; `growth_held`'s `justify_self` term dropped, as
    /// the code was, fails on *"a receipt"* — each the predicted site.
    #[test]
    fn a_grid_items_resize_receipt_outlines_the_row_it_names() {
        let mut s = grid_scene();
        set_item(&mut s.app, s.a, |i| {
            i.justify_self = Some(AlignItems::Stretch)
        });
        let tall = drawn(&s.app, s.a).height();
        s.app
            .session
            .commit(Transaction(vec![Operation::SetGeometry {
                id: s.a,
                geometry: GeometryPatch::Size(Size::new(50.0, tall)),
            }]));
        let receipt = s.app.session.flex_receipt().expect("a receipt");
        assert_eq!(
            receipt.held[0].2.justify_self,
            Some(AlignItems::Start),
            "the fixture: justify-self held"
        );
        let a = s.a;
        let mut p = Panel::new(s.app, OndinApp::inspector_item);
        p.app.session.selection.set(vec![a]);
        let shapes = p.shapes();
        let row = |head: &str| {
            shapes
                .iter()
                .find_map(|cs| match &cs.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text().starts_with(head) => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()).center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no {head} row"))
        };
        let outlined = |at: egui::Pos2| {
            shapes.iter().any(|cs| {
                matches!(
                    &cs.shape,
                    egui::epaint::Shape::Rect(r)
                        if r.stroke.color == color::ACCENT && r.rect.contains(at)
                )
            })
        };
        p.run("Resizing set justify self to start.");
        assert!(outlined(row("Justify self")), "the justify row outlined");
        assert!(!outlined(row("Align self")), "and not the align row");
    }
}
