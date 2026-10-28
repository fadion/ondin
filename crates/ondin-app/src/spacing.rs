//! **Spacing handles on the canvas** (§15 D1078): dragging a flex or grid
//! container's gaps and padding, as the Container card's fields edit them — the
//! selected container's, or the one under the pointer (§15 D1089).
//!
//! The write is the card's — one `SetDisplay` through the commit door — so the
//! binding pass (`keep_spacing_bindings`, §15 D1058), the style pass (§15
//! D1075) and the override marks judge it exactly as they judge a typed value,
//! and the floating status names whatever was detached (§15 D1071). What this
//! module adds is what is drawn, where it picks and what a drag writes.
//!
//! **The geometry is pure and in the container's local space** — [`bands`]
//! reads a `Display`, the container's box and its in-flow items' laid boxes (or
//! a grid's laid tracks) and answers the bands; [`hit`] picks one. The
//! [`OndinApp`] half maps the pointer into that space through the container's
//! world transform, so a rotated container's bands pick where they are drawn.

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::theme::color;
use ondin_core::container::{Display, LaidGrid};
use ondin_core::kurbo::{Affine, Point, Rect, Vec2};
use ondin_core::{NodeId, Operation, Transaction};

/// A band thinner than this on screen is not drawn and does not pick: at
/// that size the band is a line, and a handle on it would be guesswork.
const MIN_BAND_PX: f64 = 6.0;
/// How thick a band's pick area is on screen at least — so a gap of 0 can
/// still be grabbed and dragged open.
const MIN_PICK_PX: f64 = 8.0;
/// The pill drawn along the band under the pointer, in screen points.
const PILL_LEN: f32 = 12.0;
const PILL_THICK: f32 = 3.0;
/// The step `Shift` drags in, a nudge's coarse step.
const SHIFT_STEP: f64 = 10.0;
/// Units of spacing per local unit the pointer travels — half, so a value
/// moves one unit per two points at 100% (§15 D1094, the maintainer's
/// request: one for one was hard to set precisely).
const DRAG_RATE: f64 = 0.5;

/// One side of a container's padding, in `Display::padding`'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

/// What a band is: one side's padding, or the gaps of one axis.
///
/// **A gap band names its axis, not its role** — `ColumnGap` is the
/// horizontal gap and `RowGap` the vertical, CSS's and the engine's reading in
/// every direction (`container::Flex`'s field docs), so a column's between-items
/// gap is a `RowGap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BandKind {
    Padding(Side),
    ColumnGap,
    RowGap,
}

impl BandKind {
    /// Whether the band is dragged along x — a horizontal gap, or a left or
    /// right padding. Its cursor is then `ResizeHorizontal`.
    pub(crate) fn horizontal(self) -> bool {
        matches!(
            self,
            BandKind::ColumnGap | BandKind::Padding(Side::Left | Side::Right)
        )
    }

    /// Its slot in `Display::spacing`'s order — column gap, row gap, then the
    /// padding top, right, bottom, left (`SpacingVars`' order too).
    pub(crate) fn slot(self) -> usize {
        match self {
            BandKind::ColumnGap => 0,
            BandKind::RowGap => 1,
            BandKind::Padding(Side::Top) => 2,
            BandKind::Padding(Side::Right) => 3,
            BandKind::Padding(Side::Bottom) => 4,
            BandKind::Padding(Side::Left) => 5,
        }
    }

    /// How a pointer delta along the axis maps onto the value: a right or bottom
    /// padding grows as the pointer moves *in*, against the axis.
    fn sign(self) -> f64 {
        match self {
            BandKind::Padding(Side::Right | Side::Bottom) => -1.0,
            _ => 1.0,
        }
    }

    /// The picking order: top and bottom padding own the corners they
    /// share with the sides, and a column gap wins where it crosses a row gap.
    fn priority(self) -> u8 {
        match self {
            BandKind::Padding(Side::Top | Side::Bottom) => 0,
            BandKind::Padding(_) => 1,
            BandKind::ColumnGap => 2,
            BandKind::RowGap => 3,
        }
    }
}

/// One band, in the container's local space.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Band {
    pub kind: BandKind,
    /// The space as laid out — under `space-between` more than the stored gap
    /// Zero thick for a gap or padding of 0.
    pub rect: Rect,
    /// The stored value the label reads and the drag writes.
    pub stored: f64,
    /// The room around a zero band — its neighbours' extent along the axis —
    /// which decides whether its 8px pick strip still fits.
    pub room: f64,
    /// The label's word: *Gap* for a flex container's between-items gap, else
    /// the axis's name, or *Padding*.
    pub name: &'static str,
}

impl Band {
    /// The band's thickness along its axis.
    fn thickness(&self) -> f64 {
        if self.kind.horizontal() {
            self.rect.width()
        } else {
            self.rect.height()
        }
    }

    /// The area that picks it, at least `min` thick across its axis: a gap's
    /// strip centred on the seam, a padding's grown inward from the edge.
    fn pick(&self, min: f64) -> Rect {
        let r = self.rect;
        if self.thickness() >= min {
            return r;
        }
        let half = min / 2.0;
        match self.kind {
            BandKind::ColumnGap => {
                let c = r.center().x;
                Rect::new(c - half, r.y0, c + half, r.y1)
            }
            BandKind::RowGap => {
                let c = r.center().y;
                Rect::new(r.x0, c - half, r.x1, c + half)
            }
            BandKind::Padding(Side::Top) => Rect::new(r.x0, r.y0, r.x1, r.y0 + min),
            BandKind::Padding(Side::Bottom) => Rect::new(r.x0, r.y1 - min, r.x1, r.y1),
            BandKind::Padding(Side::Left) => Rect::new(r.x0, r.y0, r.x0 + min, r.y1),
            BandKind::Padding(Side::Right) => Rect::new(r.x1 - min, r.y0, r.x1, r.y1),
        }
    }
}

/// The bands of a container laid out in `bx` (its local box) under `display`,
/// with `items` its in-flow children's laid boxes in that space, in flow order,
/// and `grid` a grid container's laid tracks.
///
/// Padding: four bands running edge to edge, so the corners belong to two.
/// A grid: a column-gap band between each pair of column tracks, across
/// the rows' run, and a row-gap band likewise. A flex container: its
/// items split into lines where the main axis runs back (`build::flow_index`'s
/// rule), a between-items band between neighbours on a line across the line's
/// cross band, and — wrapped — a band between lines across the content's whole
/// main extent, a short last line included. Every band is where the
/// layout puts it, so a reversed direction needs nothing of its own.
pub(crate) fn bands(
    display: &Display,
    bx: Rect,
    items: &[Rect],
    grid: Option<&LaidGrid>,
) -> Vec<Band> {
    let [t, r, b, l] = display.padding().map(|v| v.max(0.0));
    let (column_gap, row_gap) = display.gaps();
    let inner = Rect::new(
        bx.x0 + l,
        bx.y0 + t,
        (bx.x1 - r).max(bx.x0 + l),
        (bx.y1 - b).max(bx.y0 + t),
    );
    let pad = |side, rect: Rect, stored, room| Band {
        kind: BandKind::Padding(side),
        rect,
        stored,
        room,
        name: "Padding",
    };
    let mut out = vec![
        pad(
            Side::Top,
            Rect::new(bx.x0, bx.y0, bx.x1, bx.y0 + t),
            t,
            inner.height(),
        ),
        pad(
            Side::Right,
            Rect::new(bx.x1 - r, bx.y0, bx.x1, bx.y1),
            r,
            inner.width(),
        ),
        pad(
            Side::Bottom,
            Rect::new(bx.x0, bx.y1 - b, bx.x1, bx.y1),
            b,
            inner.height(),
        ),
        pad(
            Side::Left,
            Rect::new(bx.x0, bx.y0, bx.x0 + l, bx.y1),
            l,
            inner.width(),
        ),
    ];
    match (display, grid) {
        (Display::Grid(_), Some(g)) => {
            let run = |spans: &[(f64, f64)]| {
                spans.iter().fold(None, |acc: Option<(f64, f64)>, s| {
                    Some(acc.map_or(*s, |(a, z)| (a.min(s.0), z.max(s.1))))
                })
            };
            let (Some(cols), Some(rows)) = (run(&g.columns.spans), run(&g.rows.spans)) else {
                return out;
            };
            for w in g.columns.spans.windows(2) {
                let ((a0, a1), (b0, b1)) = (w[0], w[1]);
                out.push(Band {
                    kind: BandKind::ColumnGap,
                    rect: Rect::new(a1, rows.0, b0.max(a1), rows.1),
                    stored: column_gap,
                    room: (a1 - a0).min(b1 - b0),
                    name: "Column gap",
                });
            }
            for w in g.rows.spans.windows(2) {
                let ((a0, a1), (b0, b1)) = (w[0], w[1]);
                out.push(Band {
                    kind: BandKind::RowGap,
                    rect: Rect::new(cols.0, a1, cols.1, b0.max(a1)),
                    stored: row_gap,
                    room: (a1 - a0).min(b1 - b0),
                    name: "Row gap",
                });
            }
        }
        (Display::Flex(f), _) => {
            let row = f.direction.is_row();
            let reversed = matches!(
                f.direction,
                ondin_core::container::FlexDirection::RowReverse
                    | ondin_core::container::FlexDirection::ColumnReverse
            );
            let wraps = f.wrap != ondin_core::container::FlexWrap::NoWrap;
            let main = |r: &Rect| if row { (r.x0, r.x1) } else { (r.y0, r.y1) };
            let cross = |r: &Rect| if row { (r.y0, r.y1) } else { (r.x0, r.x1) };
            // Lines in flow order: a new one where the main axis runs back. Each
            // is its items' main spans and the line's cross band.
            type Span = (f64, f64);
            let mut lines: Vec<(Vec<Span>, Span)> = Vec::new();
            for (k, it) in items.iter().enumerate() {
                let (m, c) = (main(it), cross(it));
                let back = k > 0 && wraps && {
                    let prev = main(&items[k - 1]);
                    if reversed {
                        m.1 > prev.0 + 1e-6
                    } else {
                        m.0 < prev.1 - 1e-6
                    }
                };
                match lines.last_mut() {
                    Some((ms, band)) if !back => {
                        ms.push(m);
                        *band = (band.0.min(c.0), band.1.max(c.1));
                    }
                    _ => lines.push((vec![m], c)),
                }
            }
            let (item_kind, line_kind) = match row {
                true => (BandKind::ColumnGap, BandKind::RowGap),
                false => (BandKind::RowGap, BandKind::ColumnGap),
            };
            let stored = |k: BandKind| match k {
                BandKind::ColumnGap => column_gap,
                _ => row_gap,
            };
            let place = |m: (f64, f64), c: (f64, f64)| match row {
                true => Rect::new(m.0, c.0, m.1, c.1),
                false => Rect::new(c.0, m.0, c.1, m.1),
            };
            for (ms, band) in &lines {
                let mut ms = ms.clone();
                ms.sort_by(|a, b| a.0.total_cmp(&b.0));
                for w in ms.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    out.push(Band {
                        kind: item_kind,
                        rect: place((a.1, b.0.max(a.1)), *band),
                        stored: stored(item_kind),
                        room: (a.1 - a.0).min(b.1 - b.0),
                        name: "Gap",
                    });
                }
            }
            let mut bands: Vec<(f64, f64)> = lines.iter().map(|(_, c)| *c).collect();
            bands.sort_by(|a, b| a.0.total_cmp(&b.0));
            let content = main(&inner);
            for w in bands.windows(2) {
                let (a, b) = (w[0], w[1]);
                out.push(Band {
                    kind: line_kind,
                    rect: place(content, (a.1, b.0.max(a.1))),
                    stored: stored(line_kind),
                    room: (a.1 - a.0).min(b.1 - b.0),
                    name: match line_kind {
                        BandKind::RowGap => "Row gap",
                        _ => "Column gap",
                    },
                });
            }
        }
        _ => {}
    }
    out
}

/// The bands worth drawing and picking at `scale` (screen points per local
/// unit along x and y): one thinner than [`MIN_BAND_PX`] is dropped on its own,
/// and a zero one only where its pick strip would crowd its neighbours —
/// twice the strip wider than their room.
pub(crate) fn shown(bands: Vec<Band>, scale: Vec2) -> Vec<Band> {
    bands
        .into_iter()
        .filter(|b| {
            let s = if b.kind.horizontal() {
                scale.x
            } else {
                scale.y
            };
            let px = b.thickness() * s;
            match b.thickness() <= 1e-9 {
                true => b.room * s >= 2.0 * MIN_PICK_PX,
                false => px >= MIN_BAND_PX,
            }
        })
        .collect()
}

/// The band at local point `p`, by [`BandKind::priority`]: none within `edge`
/// of the box's outline (the resize handles' and sides' own pick radius),
/// and **no band under a pinned child** — a layer pinned by its insets may sit
/// on the padding or across a gap, and a click there is the layer's.
/// `children` is every child's box with whether it is pinned by authored
/// insets — masks already left out by `OndinApp::spacing_subject`, a mask
/// drawing nothing of its own.
///
/// **A padding band yields to any child in that list on it; a gap band only to
/// a pinned one**, so a pinned layer over a gap keeps its click. A gap thinner
/// than `min` is a strip centred on its seam, which reaches into the in-flow
/// items either side by design, or a gap of 0 could never be dragged open.
/// `min` is the strip's thickness and `edge` the resize radius, each per axis
/// in that axis's local units.
pub(crate) fn hit(
    bands: &[Band],
    p: Point,
    bx: Rect,
    children: &[(Rect, bool)],
    min: Vec2,
    edge: Vec2,
) -> Option<usize> {
    let inside = Rect::new(
        bx.x0 + edge.x,
        bx.y0 + edge.y,
        bx.x1 - edge.x,
        bx.y1 - edge.y,
    );
    if !inside.contains(p) {
        return None;
    }
    let on_child = children.iter().any(|(c, _)| c.contains(p));
    let on_pinned = children.iter().any(|(c, pinned)| *pinned && c.contains(p));
    let mut order: Vec<usize> = (0..bands.len()).collect();
    order.sort_by_key(|i| bands[*i].kind.priority());
    order.into_iter().find(|i| {
        let b = &bands[*i];
        // A gap band runs the tracks' whole run, so a grid item spanning the
        // gap lies across it: the item, laid over the band on both sides of
        // the point, takes the press. (A gap's thin strip reaching into an item
        // beside it does not — that is the zero gap's pick.)
        let spans = || {
            children.iter().any(|(c, _)| {
                c.contains(p)
                    && match b.kind.horizontal() {
                        true => c.x0 < b.rect.x0 && c.x1 > b.rect.x1,
                        false => c.y0 < b.rect.y0 && c.y1 > b.rect.y1,
                    }
            })
        };
        let yields = match b.kind {
            BandKind::Padding(_) => on_child,
            _ => on_pinned || spans(),
        };
        if yields {
            return false;
        }
        // A padding band's strip reaches past the resize radius by `min` at
        // least, so a padding of 0 — or one thinner than the radius — can still
        // be grabbed from the inner side: padding picks only from the inner
        // part of its band, made reachable for a thin one.
        // `min` and `edge` along the band's own axis, each its screen size in
        // that axis's local units — a scaled container's 8 px are not the same
        // number of units across x as across y.
        let (m, e) = match b.kind.horizontal() {
            true => (min.x, edge.x),
            false => (min.y, edge.y),
        };
        let reach = match b.kind {
            BandKind::Padding(_) => m + e,
            _ => m,
        };
        b.pick(reach).contains(p)
    })
}

/// The slots a drag of `kind` writes under the held modifiers: a gap its
/// own axis; a padding its side, `Alt` the opposite side with it, `Alt+Shift`
/// all four.
pub(crate) fn written_slots(kind: BandKind, alt: bool, shift: bool) -> Vec<usize> {
    let opposite = match kind {
        BandKind::Padding(Side::Top) => Some(BandKind::Padding(Side::Bottom)),
        BandKind::Padding(Side::Bottom) => Some(BandKind::Padding(Side::Top)),
        BandKind::Padding(Side::Left) => Some(BandKind::Padding(Side::Right)),
        BandKind::Padding(Side::Right) => Some(BandKind::Padding(Side::Left)),
        _ => None,
    };
    match (opposite, alt, shift) {
        (Some(_), true, true) => vec![2, 3, 4, 5],
        (Some(o), true, false) => vec![kind.slot(), o.slot()],
        _ => vec![kind.slot()],
    }
}

/// The value a drag of `kind` from `base` reaches at local `delta`: the delta
/// along the band's axis (signed by [`BandKind::sign`]) at [`DRAG_RATE`], in
/// whole units, floored at 0. `step` drags in [`SHIFT_STEP`]s from `base`, as a
/// nudge steps.
///
/// **`Shift` alone steps, and not under `Alt`**: `Alt+Shift` is the all-four
/// chord, one chord cannot mean both, and all four sides is the reading a
/// designer reaches for with it.
pub(crate) fn dragged_value(kind: BandKind, base: f64, delta: Vec2, step: bool) -> f64 {
    let along = if kind.horizontal() { delta.x } else { delta.y } * kind.sign() * DRAG_RATE;
    let along = match step {
        true => (along / SHIFT_STEP).round() * SHIFT_STEP,
        false => along,
    };
    (base + along).round().max(0.0)
}

/// `literal` with `slots` set to `value` — the display one frame of the drag
/// previews and its release commits. **The literal and not the drawn**: a slot
/// the drag does not touch keeps its stored number, so a bound one stays bound
/// (`kept_spacing_vars`), and a touched bound slot detaches only if the value
/// leaves the one its variable draws.
pub(crate) fn written(literal: &Display, slots: &[usize], value: f64) -> Display {
    let mut s = literal.spacing();
    for i in slots {
        s[*i] = value;
    }
    let mut d = literal.clone();
    d.set_spacing(s);
    d
}

/// The label a drag shows: *Gap 24*, *Padding 28 · L R*,
/// *Padding 28 · all*, and under derived space *Gap 8 · laid 52*.
pub(crate) fn label(band: &Band, value: f64, slots: &[usize], laid: Option<f64>) -> String {
    let n = crate::ui::number(0)(value, 0..=0);
    let mut s = format!("{} {n}", band.name);
    if let BandKind::Padding(_) = band.kind {
        let sides = match slots.len() {
            4 => "all".to_string(),
            // In reading order — *T B*, *L R* — whichever side was grabbed, so
            // `Alt` on the right reads as it does on the left.
            _ => {
                let mut sides: Vec<usize> = slots.to_vec();
                sides.sort_by_key(|i| [0, 3, 1, 2][i - 2]);
                sides
                    .iter()
                    .map(|i| ["T", "R", "B", "L"][i - 2])
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };
        s = format!("{s} · {sides}");
    } else if let Some(laid) = laid.filter(|l| *l > value + 0.5) {
        s = format!("{s} · laid {}", crate::ui::number(0)(laid, 0..=0));
    }
    s
}

/// A selected container, as the handles read it.
pub(crate) struct Subject {
    pub id: NodeId,
    pub world: Affine,
    pub bx: Rect,
    /// The display as drawn — a bound slot its variable's number.
    pub drawn: Display,
    /// Every shown child's box but a mask's, with whether it is pinned by
    /// authored insets.
    pub children: Vec<(Rect, bool)>,
    pub bands: Vec<Band>,
    /// Screen points per local unit along x and y.
    pub scale: Vec2,
}

impl OndinApp {
    /// The flex or grid container the handles are for, its bands as the preview
    /// lays it out, or `None`: nothing laid out, a hidden container, or an edit
    /// in place.
    ///
    /// **The container under the pointer, else the one selected** (§15 D1089,
    /// the maintainer's request over D1078's (1), which had the selected one
    /// only): the topmost layer at `pointer`, when it is a container outside
    /// any group or instance ([`Self::spacing_hover`]), shows its **gap**
    /// handles on hover, so a gap is dragged without selecting its frame first —
    /// its padding only once it is selected (§15 D1093). (An occupied frame's
    /// background is not something a click selects; its gap bands take a drag
    /// all the same.) The one being dragged stays the subject whatever is under
    /// the pointer.
    pub(crate) fn spacing_subject(&self, pointer: Option<Point>) -> Option<Subject> {
        let ids = self.session.selection.ids();
        // A locked container offers no band, selected as hovered (§15 D323).
        let selected = match ids {
            [id] if !ondin_core::is_effectively_locked(&self.session.doc, *id) => Some(*id),
            _ => None,
        };
        // A container that is one of several selected keeps D1078's rule, no
        // handles: a drag on it would write it alone out of the set.
        let hovered = pointer
            .and_then(|p| self.spacing_hover(p))
            .filter(|h| ids.len() < 2 || !ids.contains(h));
        let id = match &self.drag {
            Drag::Spacing { id, .. } => *id,
            _ => hovered.or(selected)?,
        };
        // Only under a tool that selects: the press reaches `begin_select_drag`
        // for no other, so under a create tool a band drawn strong would promise
        // a drag the press turns into a shape. Here, so the drawing, the cursor,
        // the guide gate and the press all read the one answer —
        // `draw_hover_outline`'s precedent.
        if !self.selecting() || self.editing_in_place() || !self.shown_visible(id) {
            return None;
        }
        let (doc, res, ov) = self.session.render_inputs();
        let node = doc.get(id)?;
        let drawn = doc.resolved_display(id)?.into_owned();
        let bx = self.session.preview_local_box(id)?;
        let world = self.session.preview_world_transform(id)?;
        let mut items = Vec::new();
        let mut children = Vec::new();
        for c in node.children() {
            if !self.shown_visible(*c) {
                continue;
            }
            let Some(r) = self
                .session
                .preview_local_box(*c)
                .zip(self.session.preview_local_transform(*c))
                .map(|(b, t)| ondin_core::geometry::transform_rect(t, b))
            else {
                continue;
            };
            // **A mask is no child a band yields to**: it draws nothing of its
            // own, it clips, and it usually spans the whole container — counted,
            // every padding band and (as out of flow) every gap band would yield
            // to it and the handles would vanish.
            if doc.get(*c).is_some_and(|n| n.mask()) {
                continue;
            }
            let in_flow = ondin_core::build::is_flex_item(doc, *c);
            // **Pinned means pinned by insets**, not merely out of flow.
            let pinned = doc.get(*c).is_some_and(|n| n.insets().is_authored());
            children.push((r, pinned));
            if in_flow {
                items.push(r);
            }
        }
        let grid = match drawn {
            Display::Grid(_) => ov.laid_grid(doc, res, id),
            Display::Flex(_) => None,
        };
        // The preview's own display, where a drag is restyling it, so the bands
        // under the pointer follow the value being dragged.
        let shown_display = ov
            .get(id)
            .and_then(|o| o.display.clone())
            .flatten()
            .unwrap_or_else(|| drawn.clone());
        let c = world.as_coeffs();
        let ppw = self.points_per_world();
        let scale = Vec2::new(c[0].hypot(c[1]) * ppw, c[2].hypot(c[3]) * ppw);
        // **The bands a drag is writing stay, however thin**: a padding dragged
        // to 3 px at 100% would leave the list mid-drag, its strong band and its
        // label gone at the values a designer is most likely reading. Every band
        // of the dragged kind is kept — every padding for a padding drag, the
        // modifiers choosing which sides it writes frame by frame.
        let mut all = bands(&shown_display, bx, &items, grid.as_ref());
        // **An unselected container's padding is not a handle** (§15 D1093, the
        // maintainer's ruling over D1089's every band on hover): its padding is
        // where a marquee over its children starts, while a marquee rarely
        // starts in a gap between two items — so only the gaps answer the
        // hover, and the padding waits for the container to be selected —
        // through a gap drag on it too: no padding drag starts on an unselected
        // container, so sparing the dragged one would only bring its padding
        // back mid-drag.
        let hovered_only = selected != Some(id);
        if hovered_only {
            all.retain(|b| !matches!(b.kind, BandKind::Padding(_)));
        }
        let bands = match &self.drag {
            Drag::Spacing { id: d, band, .. } if *d == id => {
                let wide = shown(all.clone(), scale);
                let padding = |k: BandKind| matches!(k, BandKind::Padding(_));
                all.into_iter()
                    .filter(|b| {
                        wide.contains(b) || b.kind == *band || (padding(*band) && padding(b.kind))
                    })
                    .collect()
            }
            _ => shown(all, scale),
        };
        Some(Subject {
            id,
            world,
            bx,
            drawn,
            children,
            bands,
            scale,
        })
    }

    /// The flex or grid container directly under world point `world` — the
    /// topmost layer there, with no exact pick slop — when a click there would
    /// take it: not one inside a group or an instance, whose click takes the
    /// group (`pick_leaf`'s chain). An item or anything else on top answers
    /// `None`, so a pointer over an item never lights its container.
    fn spacing_hover(&self, world: Point) -> Option<NodeId> {
        let doc = &self.session.doc;
        let top = *ondin_core::hit_test(&self.session.resolved, doc, self.session.page, world, 0.0)
            .first()?;
        let laid = doc.resolved_display(top).is_some();
        let free = ondin_core::group_chain(doc, top, self.entered_group).is_empty()
            && ondin_core::component::instance_root(doc, top).is_none()
            && !ondin_core::is_effectively_locked(doc, top);
        (laid && free).then_some(top)
    }

    /// The band under screen point `screen`, with its subject — what a press
    /// there would grab and what the cursor answers for.
    pub(crate) fn spacing_at(
        &self,
        screen: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> Option<(Subject, usize)> {
        if !matches!(self.drag, Drag::None) {
            return None;
        }
        let world = self.to_world(screen, rect, ppp);
        let s = self.spacing_subject(Some(world))?;
        let local = s.world.inverse() * world;
        let min = Vec2::new(
            MIN_PICK_PX / s.scale.x.max(1e-9),
            MIN_PICK_PX / s.scale.y.max(1e-9),
        );
        let edge = Vec2::new(
            f64::from(crate::canvas::HANDLE_PICK_PX) / s.scale.x.max(1e-9),
            f64::from(crate::canvas::HANDLE_PICK_PX) / s.scale.y.max(1e-9),
        );
        let i = hit(&s.bands, local, s.bx, &s.children, min, edge)?;
        Some((s, i))
    }

    /// Start a band drag at the press `grab`, or `false` where it is over no
    /// band — the press path's next arm then takes it.
    pub(crate) fn begin_spacing_drag(
        &mut self,
        grab: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        let Some((s, i)) = self.spacing_at(grab, rect, ppp) else {
            return false;
        };
        let band = &s.bands[i];
        self.drag = Drag::Spacing {
            id: s.id,
            band: band.kind,
            anchor: s.world.inverse() * self.to_world(grab, rect, ppp),
            // From the drawn value, so a bound slot's drag starts at the number
            // its variable lays out.
            base: s.drawn.spacing()[band.kind.slot()],
        };
        true
    }

    /// The transaction a band drag at world point `world` previews and commits,
    /// with the value and slots it writes; `None` where the container is gone.
    pub(crate) fn band_drag_tx(
        &self,
        world: Point,
        shift: bool,
        alt: bool,
    ) -> Option<(Transaction, f64, Vec<usize>)> {
        let Drag::Spacing {
            id,
            band,
            anchor,
            base,
        } = &self.drag
        else {
            return None;
        };
        // In the committed frame the anchor was taken in, never the preview's:
        // a hugging container its parent aligns to the end or the centre moves
        // its origin as the drag widens it, and the pointer's local x would
        // grow with it — the value creeping on with the hand still.
        let node_world = self.session.resolved.world_transform(*id)?;
        let doc = &self.session.doc;
        let node = doc.get(*id)?;
        let literal = node.display()?.clone();
        let delta = node_world.inverse() * world - *anchor;
        let slots = written_slots(*band, alt, shift);
        let step = shift && !(alt && matches!(band, BandKind::Padding(_)));
        let value = dragged_value(*band, *base, delta, step);
        // **The drag cuts a binding itself** where it moves a bound slot off the
        // number its variable draws. Left to `keep_spacing_bindings`, a drag to
        // 0 matched the bound slot's literal 0 — the pass's *"written its own
        // literal"* half, for writers carrying the other slots forward — so the
        // binding stayed and the slot sprang back to its variable on release
        // (§15 D1090). A transaction setting the bindings is left alone by that
        // pass, so the slots kept bound are written their literal here.
        let bound = node.spacing_vars();
        let drawn = doc.resolved_display(*id)?.spacing();
        let mut vars = bound;
        for k in &slots {
            if bound[*k].is_some() && value != drawn[*k] {
                vars[*k] = None;
            }
        }
        let mut display = written(&literal, &slots, value);
        let mut ops = Vec::new();
        if vars != bound {
            let mut s = display.spacing();
            for k in &slots {
                if vars[*k].is_some() {
                    s[*k] = literal.spacing()[*k];
                }
            }
            display.set_spacing(s);
            ops.push(Operation::SetSpacingVars { id: *id, vars });
        }
        ops.push(Operation::SetDisplay {
            id: *id,
            display: Some(display),
        });
        Some((Transaction(ops), value, slots))
    }

    /// The variables `tx` — a band drag's, [`Self::band_drag_tx`] — cuts, by
    /// name, for the floating status. The commit says what its own passes cut
    /// (§15 D1071) and says nothing of a binding the edit sets itself, which is
    /// a chip's asked-for detach; a drag's cut is a detach the user wrote
    /// over, so the release says it.
    pub(crate) fn band_drag_cut(&self, tx: &Transaction) -> Vec<String> {
        let doc = &self.session.doc;
        let mut names: Vec<String> =
            tx.0.iter()
                .filter_map(|op| match op {
                    Operation::SetSpacingVars { id, vars } => Some((doc.get(*id)?, vars)),
                    _ => None,
                })
                .flat_map(|(node, vars)| {
                    let had = node.spacing_vars();
                    (0..6)
                        .filter(move |k| vars[*k].is_none())
                        .filter_map(move |k| had[k])
                })
                .filter_map(|v| doc.variables().get(v).map(|x| x.name.clone()))
                .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The handles' drawing: nothing at rest; with the pointer inside the
    /// subject container ([`Self::spacing_subject`]) every band faint in `GRID_GAP`'s tint — a grid's gaps
    /// are already drawn by its track lines, so only its padding is added — the
    /// band under the pointer strong with its pill; while dragging only the
    /// bands being written, strong, and the label beside the pointer. A bound
    /// band's label shows on hover, its variable as a capsule.
    pub(crate) fn draw_spacing_bands(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if self.mode != crate::input::Mode::Normal {
            return;
        }
        let dragging = matches!(self.drag, Drag::Spacing { .. });
        if !dragging && (!matches!(self.drag, Drag::None) || self.chrome_hidden()) {
            return;
        }
        let Some(pointer) = painter.ctx().input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let world_ptr = self.to_world(pointer, rect, ppp);
        let Some(s) = self.spacing_subject(Some(world_ptr)) else {
            return;
        };
        let local = s.world.inverse() * world_ptr;
        if !dragging && !s.bx.contains(local) {
            return;
        }
        let hue = crate::canvas::chrome_hue(&self.session.doc, s.id);
        let quad = |r: Rect| self.local_quad(r, s.world, rect, ppp);
        let fill = |r: Rect, c: egui::Color32| {
            painter.add(egui::Shape::convex_polygon(
                quad(r).to_vec(),
                c,
                egui::Stroke::NONE,
            ));
        };
        let strong = color::SPACING_STRONG;
        let (shift, alt) = painter
            .ctx()
            .input(|i| (i.modifiers.shift, i.modifiers.alt));
        if dragging {
            let Some((_, value, slots)) = self.band_drag_tx(world_ptr, shift, alt) else {
                return;
            };
            let Drag::Spacing { band, .. } = &self.drag else {
                return;
            };
            let lit: Vec<&Band> = s
                .bands
                .iter()
                .filter(|b| slots.contains(&b.kind.slot()))
                .collect();
            for b in &lit {
                fill(b.rect, strong);
            }
            if let Some(b) = lit.iter().find(|b| b.kind == *band).or(lit.first()) {
                self.spacing_hatch(painter, &s, b, value, rect, ppp);
                let laid = (b.kind.slot() < 2).then(|| b.thickness());
                paint_label(painter, pointer, &label(b, value, &slots, laid), None, hue);
            }
            return;
        }
        let grid = matches!(s.drawn, Display::Grid(_));
        let hovered = self.spacing_at(pointer, rect, ppp).map(|(_, i)| i);
        for (i, b) in s.bands.iter().enumerate() {
            if Some(i) == hovered {
                continue;
            }
            if grid && !matches!(b.kind, BandKind::Padding(_)) {
                continue;
            }
            fill(b.rect, color::GRID_GAP);
        }
        let Some(i) = hovered else {
            return;
        };
        let b = &s.bands[i];
        fill(b.rect, strong);
        self.spacing_hatch(painter, &s, b, b.stored, rect, ppp);
        paint_pill(painter, quad(b.rect), b.kind, hue);
        // A bound slot names its variable before any drag.
        if let Some(var) = self
            .session
            .doc
            .get(s.id)
            .and_then(|n| n.spacing_vars()[b.kind.slot()])
            && let Some(v) = self.session.doc.variables().get(var)
        {
            paint_label(painter, pointer, b.name, Some(&v.name), hue);
        }
    }

    /// The arrows a band's drag shows: along the band's axis as the container's
    /// world transform turns it, so a rotated container's arrows point the way
    /// the drag moves (`resize_icon`'s octants).
    pub(crate) fn spacing_cursor(&self, id: NodeId, band: BandKind) -> egui::CursorIcon {
        let Some(w) = self.session.preview_world_transform(id) else {
            return egui::CursorIcon::Default;
        };
        let c = w.as_coeffs();
        let (x, y) = match band.horizontal() {
            true => (c[0], c[1]),
            false => (c[2], c[3]),
        };
        crate::canvas::resize_icon(egui::vec2(x as f32, y as f32))
    }

    /// The derived-space hatch: where a gap lays out wider than it stores, the stored part
    /// marked at the band's leading edge, so the label's two numbers read.
    fn spacing_hatch(
        &self,
        painter: &egui::Painter,
        s: &Subject,
        b: &Band,
        stored: f64,
        rect: egui::Rect,
        ppp: f32,
    ) {
        if matches!(b.kind, BandKind::Padding(_)) || b.thickness() <= stored + 0.5 {
            return;
        }
        let r = b.rect;
        let part = match b.kind.horizontal() {
            true => Rect::new(r.x0, r.y0, r.x0 + stored, r.y1),
            false => Rect::new(r.x0, r.y0, r.x1, r.y0 + stored),
        };
        // Diagonals every 4 screen points across the stored part, clipped to it.
        let step = 4.0 / s.scale.x.min(s.scale.y).max(1e-9);
        let ink = egui::Stroke::new(1.0, color::SPACING_HATCH);
        let mut c = part.x0 + part.y0;
        while c < part.x1 + part.y1 {
            // The line x + y = c clipped to `part`.
            let a = Point::new((c - part.y0).clamp(part.x0, part.x1), 0.0);
            let a = Point::new(a.x, c - a.x);
            let z = Point::new((c - part.y1).clamp(part.x0, part.x1), 0.0);
            let z = Point::new(z.x, c - z.x);
            painter.line_segment(
                [
                    self.to_screen(s.world * a, rect, ppp),
                    self.to_screen(s.world * z, rect, ppp),
                ],
                ink,
            );
            c += step;
        }
    }
}

/// The pill at a band's middle: lying along the band, across the
/// direction it drags.
fn paint_pill(painter: &egui::Painter, quad: [egui::Pos2; 4], kind: BandKind, hue: egui::Color32) {
    let centre = egui::pos2(
        quad.iter().map(|p| p.x).sum::<f32>() / 4.0,
        quad.iter().map(|p| p.y).sum::<f32>() / 4.0,
    );
    let size = match kind.horizontal() {
        true => egui::vec2(PILL_THICK, PILL_LEN),
        false => egui::vec2(PILL_LEN, PILL_THICK),
    };
    painter.rect_filled(
        egui::Rect::from_center_size(centre, size),
        egui::CornerRadius::same((PILL_THICK / 2.0).round() as u8),
        hue,
    );
}

/// The value label beside the pointer, drawn as the size badge's pill;
/// `var` a bound slot's variable, as a capsule after the word. The gradient
/// handles' readout too.
pub(crate) fn paint_label(
    painter: &egui::Painter,
    at: egui::Pos2,
    text: &str,
    var: Option<&str>,
    hue: egui::Color32,
) {
    const PAD: egui::Vec2 = egui::vec2(7.0, 3.0);
    let ink = match hue == color::COMPONENT {
        true => color::COMPONENT_INK,
        false => color::BADGE_INK,
    };
    let font = egui::FontId::proportional(9.5);
    let word = painter.layout_no_wrap(text.to_owned(), font.clone(), ink);
    let cap = var.map(|v| painter.layout_no_wrap(v.to_owned(), font, ink));
    let cap_w = cap.as_ref().map_or(0.0, |g| g.size().x + 10.0);
    let size = egui::vec2(word.size().x + cap_w, word.size().y) + PAD * 2.0;
    let box_ = egui::Rect::from_min_size(at + egui::vec2(14.0, 14.0), size);
    painter.rect_filled(box_, egui::CornerRadius::same(4), hue);
    let word_w = word.size().x;
    painter.galley(box_.min + PAD, word, ink);
    if let Some(g) = cap {
        let r = egui::Rect::from_min_size(
            box_.min + PAD + egui::vec2(word_w + 6.0, -1.0),
            g.size() + egui::vec2(8.0, 2.0),
        );
        painter.rect_filled(
            r,
            egui::CornerRadius::same((r.height() / 2.0) as u8),
            ink.gamma_multiply(0.18),
        );
        painter.galley(r.min + egui::vec2(4.0, 1.0), g, ink);
    }
}

/// The handles driven through the whole canvas with real pointer events (§15
/// D1078) — press, drag, release through `canvas_ui`, the cursor read off the
/// frame's output. Plain backticks, per §15 D319.
#[cfg(test)]
mod canvas_tests {
    use crate::app::OndinApp;
    use ondin_core::container::{AlignItems, Display, Flex};
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    /// A selected 400 × 200 row at the origin — padding 20, gap 10, items to the
    /// start — of a 40, a 60 and a 20 wide, 30 tall: drawn at x 20–60, 70–130
    /// and 140–160, y 20–50, so the first gap band is x 60–70. Camera on the
    /// row's centre at 100%.
    fn app_with_a_row(ctx: &egui::Context) -> (OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0xBE);
        let root = ids.mint();
        let row = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: row,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }];
        for (i, w) in [40.0, 60.0, 20.0].into_iter().enumerate() {
            ops.push(Operation::CreateNode {
                id: ids.mint(),
                parent: row,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(w, 30.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::SetDisplay {
            id: row,
            display: Some(Display::Flex(Flex {
                column_gap: 10.0,
                padding: [20.0; 4],
                align_items: AlignItems::Start,
                ..Default::default()
            })),
        });
        let mut doc = Document::new(root);
        doc.apply(&Transaction(ops)).expect("the row");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(200.0, 100.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set_one(row);
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new(), Default::default());
        }
        (app, row)
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) -> egui::FullOutput {
        let time = ctx.input(|i| i.time) + 0.1;
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                modifiers,
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        )
    }

    fn button(at: egui::Pos2, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        }
    }

    /// Press at world `from`, drag to `to` in eight steps holding `modifiers`,
    /// and release unless `hold` — then the drag is left in flight.
    fn drag(
        ctx: &egui::Context,
        app: &mut OndinApp,
        from: Point,
        to: Point,
        modifiers: egui::Modifiers,
        hold: bool,
    ) {
        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, 1.0);
        let start = at(app, from);
        frame(ctx, app, vec![egui::Event::PointerMoved(start)], modifiers);
        frame(ctx, app, vec![button(start, true, modifiers)], modifiers);
        for step in 1..=8 {
            let p = at(app, from + (to - from) * (step as f64 / 8.0));
            frame(ctx, app, vec![egui::Event::PointerMoved(p)], modifiers);
        }
        if hold {
            return;
        }
        frame(
            ctx,
            app,
            vec![button(at(app, to), false, modifiers)],
            modifiers,
        );
        frame(ctx, app, Vec::new(), Default::default());
    }

    fn spacing(app: &OndinApp, row: NodeId) -> [f64; 6] {
        app.session
            .doc
            .get(row)
            .unwrap()
            .display()
            .unwrap()
            .spacing()
    }

    /// **Dragging a gap band writes the gap, one undo step**: the band
    /// between the first two items grabbed at x 65 and dragged 16 to the right
    /// — 8 at the drag's half rate (§15 D1094) — makes the gap 18, every gap of
    /// the row with it, and undo puts back 10.
    ///
    /// Flip, run: `begin_spacing_drag` answering `false` leaves the press to the
    /// move arm — fails *"the gap took the drag"*, still 10. `DRAG_RATE` back at
    /// 1 fails there too, 26.
    #[test]
    fn dragging_a_gap_band_writes_the_gap() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            false,
        );
        assert_eq!(spacing(&app, row)[0], 18.0, "the gap took the drag");
        assert_eq!(
            app.session.doc.get(row).unwrap().transform(),
            Affine::IDENTITY,
            "the row did not move"
        );
        app.undo();
        assert_eq!(spacing(&app, row)[0], 10.0);
    }

    /// **An unselected container's gap drags on hover** (§15 D1089): with
    /// nothing selected a drag across the row's first gap writes the gap and
    /// leaves the selection alone, and the cursor there is the horizontal
    /// arrows.
    ///
    /// Flip, run: `spacing_subject` ignoring the hover (the selected container
    /// only, as D1078 had it) fails *"the hovered gap took the drag"*, still 10.
    #[test]
    fn an_unselected_containers_gap_drags_on_hover() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        app.session.selection.clear();
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            false,
        );
        assert_eq!(spacing(&app, row)[0], 18.0, "the hovered gap took the drag");
        assert!(app.session.selection.is_empty(), "a drag selects nothing");
        let at = app.to_screen(Point::new(65.0, 35.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], none);
        let cursor = frame(&ctx, &mut app, Vec::new(), none)
            .platform_output
            .cursor_icon;
        assert_eq!(
            cursor,
            egui::CursorIcon::ResizeHorizontal,
            "the arrows on hover"
        );
    }

    /// **An unselected container's padding starts a marquee; selected, it is a
    /// handle** (§15 D1093): with nothing selected, a press in the row's top
    /// padding and dragged starts a marquee and its gap is still a band; with
    /// the row selected, the same padding is a band.
    ///
    /// Flip, run: the padding filter off (`hovered_only` false) fails *"the
    /// press in an unselected row's padding starts a marquee"*.
    #[test]
    fn an_unselected_containers_padding_is_a_marquee_start() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        app.session.selection.clear();
        let none = Default::default();
        let padding = app.to_screen(Point::new(200.0, 14.0), RECT, 1.0);
        let gap = app.to_screen(Point::new(65.0, 35.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(gap)], none);
        assert!(
            app.spacing_at(gap, RECT, 1.0).is_some(),
            "an unselected row's gap is still a band"
        );
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 14.0),
            Point::new(260.0, 60.0),
            none,
            true,
        );
        assert!(
            matches!(app.drag, crate::preview::Drag::Marquee { .. }),
            "the press in an unselected row's padding starts a marquee"
        );
        let end = app.to_screen(Point::new(260.0, 60.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![button(end, false, none)], none);
        frame(&ctx, &mut app, Vec::new(), none);

        app.session.selection.set_one(row);
        frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(padding)],
            none,
        );
        assert!(
            app.spacing_at(padding, RECT, 1.0).is_some(),
            "selected, its padding is a band"
        );
    }

    /// **A click in an empty container's padding still selects it** (§15
    /// D1089): an empty frame is directly clickable (`pick_leaf`). Since §15
    /// D1093 an unselected container's padding is no band at all; before it,
    /// the band its hover showed did not take the click either — a press
    /// released where it was pressed is a click, not a band drag.
    ///
    /// The band does not take the click, and there is no selection arm in the
    /// band release's no-op path: one added there and flipped out leaves this
    /// green, the click path selecting the row on its own.
    #[test]
    fn a_click_in_an_empty_containers_padding_selects_it() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let items = app.session.doc.get(row).unwrap().children().to_vec();
        app.session.commit(Transaction(
            items
                .into_iter()
                .map(|id| Operation::DeleteNode { id })
                .collect(),
        ));
        app.session.selection.clear();
        let none = Default::default();
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new(), none);
        }
        let at = app.to_screen(Point::new(200.0, 10.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)], none);
        assert!(
            app.spacing_at(at, RECT, 1.0).is_none(),
            "an unselected container's padding is no band (§15 D1093)"
        );
        frame(&ctx, &mut app, vec![button(at, true, none)], none);
        frame(&ctx, &mut app, vec![button(at, false, none)], none);
        frame(&ctx, &mut app, Vec::new(), none);
        assert_eq!(
            app.session.selection.ids(),
            [row],
            "the click selects the empty row"
        );
    }

    /// **The modifiers choose the sides and the step**: `Shift` on a gap steps by 10 — a 13-point drag, 6.5
    /// at half rate (§15 D1094), is +10 — `Alt` on the left padding writes the
    /// right with it, and `Alt+Shift` all four, in whole units with no step.
    ///
    /// Flip, run: the band's press placed below `begin_select_drag`'s
    /// `Alt`+`Shift` refusal (spelled as the band arm skipped under that chord)
    /// fails *"alt+shift: all four, no step"* — the chord started no drag.
    #[test]
    fn the_modifiers_choose_the_sides_and_the_step() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(78.0, 35.0),
            shift,
            false,
        );
        assert_eq!(spacing(&app, row)[0], 20.0, "shift steps by 10");

        let alt = egui::Modifiers {
            alt: true,
            ..Default::default()
        };
        drag(
            &ctx,
            &mut app,
            Point::new(14.0, 120.0),
            Point::new(30.0, 120.0),
            alt,
            false,
        );
        assert_eq!(
            &spacing(&app, row)[2..],
            &[20.0, 28.0, 20.0, 28.0],
            "alt: left and right"
        );

        let both = egui::Modifiers {
            alt: true,
            shift: true,
            ..Default::default()
        };
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 14.0),
            Point::new(200.0, 40.0),
            both,
            false,
        );
        assert_eq!(
            &spacing(&app, row)[2..],
            &[33.0; 4],
            "alt+shift: all four, no step"
        );
    }

    /// **A bound gap detaches when released off its variable and says so, and
    /// stays bound released on it**: the gap bound to *space/md* (10);
    /// a drag back to where it started commits nothing, a drag to 18 cuts the
    /// binding and the floating status names it.
    ///
    /// ⚠️ **Two defences, so a flip of either alone does not bite**: the
    /// release's guard compared against the stored literal (0) in place of the
    /// drawn value commits the drag — and `commit_inner`'s no-op test then drops
    /// it, `keep_spacing_bindings` having written the bound slot back to its
    /// literal. Run, green. The undo-depth assertion is what either one keeps.
    #[test]
    fn a_bound_gap_detaches_only_off_its_variable() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "space/md".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(10.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        let ops = ondin_core::build::bind_spacing_ops(
            &app.session.doc,
            row,
            [Some(var), None, None, None, None, None],
        );
        assert!(app.session.commit(Transaction(ops)));
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let bound = |app: &OndinApp| app.session.doc.get(row).unwrap().spacing_vars()[0];
        assert_eq!(bound(&app), Some(var), "the fixture: bound");
        let undo_depth = |app: &OndinApp| app.session.history.undo_depth();
        let before = undo_depth(&app);

        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(65.0, 40.0),
            none,
            false,
        );
        assert_eq!(bound(&app), Some(var));
        assert_eq!(
            undo_depth(&app),
            before,
            "a release on the variable's number is no undo step"
        );

        app.session.info("");
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            false,
        );
        assert_eq!(bound(&app), None, "released off its number, detached");
        assert_eq!(spacing(&app, row)[0], 18.0);
        assert_eq!(
            app.session.status().text,
            "Detached space/md",
            "and says so"
        );
    }

    /// **A bound padding dragged to 0 is 0** (§15 D1090): the top padding bound
    /// to *space/md* (10) — its literal 0 — dragged up past the edge previews 0
    /// while held, and released stays 0, the binding cut and said.
    ///
    /// Flip, run: `band_drag_tx` leaving the cut to `keep_spacing_bindings`
    /// (no `SetSpacingVars`) fails *"the preview lays out 0"* — 10, the written
    /// 0 read as the slot's own literal and the binding kept — and, that
    /// assertion set aside, *"released at 0, detached"*. The renderer's
    /// `SetDisplay` arm judging the kept bindings against the document in place
    /// of the pending ones fails *"the preview lays out 0"*, 10.
    #[test]
    fn a_bound_padding_dragged_to_zero_is_zero() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let var = bind_top_padding(&ctx, &mut app, row);
        let bound = |app: &OndinApp| app.session.doc.get(row).unwrap().spacing_vars()[2];
        assert_eq!(bound(&app), Some(var), "the fixture: bound");
        assert_eq!(spacing(&app, row)[2], 0.0, "the fixture: its literal is 0");

        app.session.info("");
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 12.0),
            Point::new(200.0, -20.0),
            none,
            true,
        );
        assert!(
            matches!(app.drag, crate::preview::Drag::Spacing { .. }),
            "the fixture: the padding band took the press"
        );
        let previewed = {
            let (_, _, ov) = app.session.render_inputs();
            ov.get(row)
                .and_then(|o| o.display.clone())
                .flatten()
                .map(|d| d.spacing()[2])
        };
        assert_eq!(previewed, Some(0.0), "the preview lays out 0");
        let end = app.to_screen(Point::new(200.0, -20.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![button(end, false, none)], none);
        frame(&ctx, &mut app, Vec::new(), none);
        assert_eq!(bound(&app), None, "released at 0, detached");
        assert_eq!(spacing(&app, row)[2], 0.0);
        assert_eq!(
            app.session.status().text,
            "Detached space/md",
            "and says so"
        );
    }

    /// **The maintainer's case: a layout-styled container's bound padding
    /// dragged to 0** (§15 D1090) — the row the user of a layout style holding
    /// its top padding's binding, as *Stack/Column* in the report holds its
    /// row gap's and paddings'. The drag cuts the style and the binding, the
    /// padding stays 0, and the status counts both.
    ///
    /// Flip, run: `band_drag_tx` with no `SetSpacingVars` fails first at *"the
    /// style cut"* — the release wrote nothing at all, the 0 read as the bound
    /// slot's literal and the style's value unchanged, which is the report's
    /// spring-back exactly.
    #[test]
    fn a_styled_bound_padding_dragged_to_zero_is_zero() {
        use ondin_core::style::{self, StyleSlot};
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let var = bind_top_padding(&ctx, &mut app, row);
        let s = app.session.ids.mint_style();
        let ops = style::create_ops(
            &app.session.doc,
            s,
            "Stack/Column",
            row,
            StyleSlot::Layout,
            &[row],
        )
        .expect("the row has a layout");
        assert!(app.session.commit(Transaction(ops)));
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let node = |app: &OndinApp| app.session.doc.get(row).unwrap().clone();
        assert_eq!(node(&app).styles().layout, Some(s), "the fixture: styled");
        assert_eq!(node(&app).spacing_vars()[2], Some(var), "and bound");

        app.session.info("");
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 12.0),
            Point::new(200.0, -20.0),
            none,
            false,
        );
        assert_eq!(node(&app).styles().layout, None, "the style cut");
        assert_eq!(node(&app).spacing_vars()[2], None, "the binding cut");
        assert_eq!(spacing(&app, row)[2], 0.0, "released at 0");
        assert_eq!(app.session.status().text, "Detached 2 styles and variables");
    }

    /// Bind `row`'s top padding to a new *space/md* (10, one mode), answering
    /// the variable.
    fn bind_top_padding(ctx: &egui::Context, app: &mut OndinApp, row: NodeId) -> ondin_core::VarId {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "space/md".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(10.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        let ops = ondin_core::build::bind_spacing_ops(
            &app.session.doc,
            row,
            [None, None, Some(var), None, None, None],
        );
        assert!(app.session.commit(Transaction(ops)));
        frame(ctx, app, Vec::new(), Default::default());
        var
    }

    /// **A band dragged thin stays drawn while it is dragged**: the top padding
    /// held mid-drag at 3 — under the 6-point floor at 100% — is still among
    /// the subject's bands, so its strong band and label stay; let go, it is
    /// dropped as any thin band is. And **under a create tool there are no
    /// bands** at all, the press drawing a shape there.
    ///
    /// Flip, run: the dragged kind's exemption taken out fails *"held thin, still
    /// drawn"*.
    #[test]
    fn a_band_dragged_thin_stays_while_dragged() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_row(&ctx);
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(200.0, 14.0),
            Point::new(200.0, -20.0),
            none,
            true,
        );
        let top = |app: &OndinApp| {
            app.spacing_subject(None).is_some_and(|s| {
                s.bands
                    .iter()
                    .any(|b| b.kind == super::BandKind::Padding(super::Side::Top))
            })
        };
        assert!(top(&app), "held thin, still drawn");
        let end = app.to_screen(Point::new(200.0, -20.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![button(end, false, none)], none);
        frame(&ctx, &mut app, Vec::new(), none);
        assert!(!top(&app), "let go at 3, dropped");
        app.tool = crate::tools::Tool::Rect;
        assert!(
            app.spacing_subject(None).is_none(),
            "a create tool: no bands"
        );
    }

    /// **A cancel abandons a band drag** (§9.4): the gap dragged to 18 and
    /// `cancel_gesture` run before the release — what `Escape` and the right
    /// button both reach — leaves it 10, nothing committed. A new `Drag` variant
    /// is inside `cancel_gesture`'s early return with no term of its own.
    #[test]
    fn a_cancel_abandons_a_band_drag() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            true,
        );
        assert!(
            matches!(app.drag, crate::preview::Drag::Spacing { .. }),
            "in flight"
        );
        assert!(app.cancel_gesture(&ctx), "the cancel took the drag");
        let end = app.to_screen(Point::new(81.0, 35.0), RECT, 1.0);
        frame(&ctx, &mut app, vec![button(end, false, none)], none);
        frame(&ctx, &mut app, Vec::new(), none);
        assert_eq!(spacing(&app, row)[0], 10.0, "nothing committed");
    }

    /// **Bands are drawn on hover, not at rest**: with the pointer off
    /// the selected row no band's tint is in the frame's shapes; over its gap
    /// the faint bands and the strong one are.
    ///
    /// Flip, run: `draw_spacing_bands` without its pointer-inside test fails on
    /// *"nothing at rest"*.
    #[test]
    fn bands_are_drawn_on_hover_not_at_rest() {
        let ctx = egui::Context::default();
        let (mut app, _) = app_with_a_row(&ctx);
        let fills = |out: &egui::FullOutput, c: egui::Color32| {
            out.shapes
                .iter()
                .filter(|s| matches!(&s.shape, egui::Shape::Path(p) if p.fill == c))
                .count()
        };
        let at = |app: &mut OndinApp, w: Point| {
            let p = app.to_screen(w, RECT, 1.0);
            frame(
                &ctx,
                app,
                vec![egui::Event::PointerMoved(p)],
                Default::default(),
            );
            frame(&ctx, app, Vec::new(), Default::default())
        };
        let rest = at(&mut app, Point::new(600.0, 100.0));
        assert_eq!(
            fills(&rest, crate::theme::color::GRID_GAP),
            0,
            "nothing at rest"
        );
        let hover = at(&mut app, Point::new(65.0, 35.0));
        assert_eq!(
            fills(&hover, crate::theme::color::SPACING_STRONG),
            1,
            "the band under it"
        );
        assert_eq!(
            fills(&hover, crate::theme::color::GRID_GAP),
            5,
            "four paddings, one gap"
        );
    }

    /// The display the row is laid out under, changed by `f` and committed.
    fn restyle(app: &mut OndinApp, row: NodeId, f: impl FnOnce(&mut Display)) {
        let mut d = app.session.doc.get(row).unwrap().display().unwrap().clone();
        f(&mut d);
        assert!(app.session.commit(Transaction(vec![Operation::SetDisplay {
            id: row,
            display: Some(d),
        }])));
    }

    fn texts(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    fn hatches(out: &egui::FullOutput) -> usize {
        out.shapes
            .iter()
            .filter(|s| {
                matches!(&s.shape, egui::Shape::LineSegment { stroke, .. }
                    if stroke.color == crate::theme::color::SPACING_HATCH)
            })
            .count()
    }

    /// **Derived space: the hatch and the label**: under
    /// `space-between` at gap 8 the first gap lays out 120 wide — (360 − 120 −
    /// 16) / 2 + 8 — so hovering it hatches the stored part, and dragging it
    /// shows *Gap 8 · laid 120*; at the start, where the gap lays out as it
    /// stores, there is no hatch.
    ///
    /// Flip, run: `spacing_hatch` returning at once fails *"hatched where it
    /// lays out wider"*.
    #[test]
    fn derived_space_is_hatched_and_labelled() {
        use ondin_core::container::JustifyContent;
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let hover = |app: &mut OndinApp, w: Point| {
            let p = app.to_screen(w, RECT, 1.0);
            frame(
                &ctx,
                app,
                vec![egui::Event::PointerMoved(p)],
                Default::default(),
            );
            frame(&ctx, app, Vec::new(), Default::default())
        };
        assert_eq!(
            hatches(&hover(&mut app, Point::new(65.0, 35.0))),
            0,
            "no derived space"
        );
        restyle(&mut app, row, |d| {
            if let Display::Flex(f) = d {
                f.justify_content = JustifyContent::SpaceBetween;
                f.column_gap = 8.0;
            }
        });
        frame(&ctx, &mut app, Vec::new(), Default::default());
        assert!(
            hatches(&hover(&mut app, Point::new(120.0, 35.0))) > 0,
            "hatched where it lays out wider"
        );
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(120.0, 35.0),
            Point::new(120.0, 45.0),
            none,
            true,
        );
        let out = frame(&ctx, &mut app, Vec::new(), none);
        assert!(
            texts(&out).iter().any(|t| t == "Gap 8 · laid 120"),
            "the label: {:?}",
            texts(&out)
        );
    }

    /// **A grid's column gap drags end to end**: a grid of two 100-wide
    /// columns at gap 10 inside padding 20 — the gap band x 120–130 — dragged
    /// 16 to the right (8 at half rate) is a gap of 18, the row gap untouched.
    ///
    /// Flip, run: `bands`' grid arm switched off fails here at 10 (and
    /// `a_grids_bands_are_between_its_tracks` on *"a column gap"*).
    #[test]
    fn a_grids_column_gap_drags() {
        use ondin_core::container::{Grid, Track, TrackBreadth as B, TrackSize as S};
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let px = |v| Track::Size(S::Breadth(B::Px(v)));
        restyle(&mut app, row, |d| {
            *d = Display::Grid(Grid {
                columns: vec![px(100.0), px(100.0)],
                rows: vec![px(50.0), px(50.0)],
                column_gap: 10.0,
                row_gap: 6.0,
                padding: [20.0; 4],
                ..Default::default()
            });
        });
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(125.0, 40.0),
            Point::new(141.0, 40.0),
            none,
            false,
        );
        assert_eq!(&spacing(&app, row)[..2], &[18.0, 6.0]);
    }

    /// **A selected, then locked, layer is not dragged about** (§15 D323, D348):
    /// its gap, its right side and its body each dragged, and nothing commits.
    /// The control, the same three on the row unlocked, each commits.
    ///
    /// flips: `Handles::armed`'s lock answer removed → fails *"the side"*;
    /// `spacing_subject` keeping a locked selection → fails *"the gap"*; the
    /// move's `selection_locked` refusal removed → fails *"the gap"* too, the
    /// refused band's press falling through to a move.
    #[test]
    fn a_selected_locked_layer_takes_no_drag() {
        let ctx = egui::Context::default();
        let none = Default::default();
        let drags = [
            ("the gap", Point::new(65.0, 35.0), Point::new(81.0, 35.0)),
            (
                "the side",
                Point::new(400.0, 100.0),
                Point::new(450.0, 100.0),
            ),
            (
                "the body",
                Point::new(250.0, 150.0),
                Point::new(290.0, 170.0),
            ),
        ];
        for (what, from, to) in drags {
            let (mut app, row) = app_with_a_row(&ctx);
            let depth = app.session.history.undo_depth();
            drag(&ctx, &mut app, from, to, none, false);
            assert_eq!(
                app.session.history.undo_depth(),
                depth + 1,
                "the fixture: {what} drags unlocked"
            );
            let (mut app, row2) = app_with_a_row(&ctx);
            assert_eq!(row, row2);
            assert!(app.session.commit(Transaction(vec![Operation::SetLocked {
                id: row,
                locked: true,
            }])));
            app.session.selection.set_one(row);
            frame(&ctx, &mut app, Vec::new(), Default::default());
            let depth = app.session.history.undo_depth();
            drag(&ctx, &mut app, from, to, none, false);
            assert_eq!(app.session.history.undo_depth(), depth, "{what}");
        }
    }

    /// **A locked line's endpoints refuse too** (§15 D323): a lone line's end
    /// dragged commits unlocked and nothing locked — the endpoints are the
    /// line's handles.
    ///
    /// flip: `grab_line_end`'s lock refusal removed → fails *"the locked end"*.
    #[test]
    fn a_locked_lines_endpoints_take_no_drag() {
        let ctx = egui::Context::default();
        let none = Default::default();
        for locked in [false, true] {
            let (mut app, _) = app_with_a_row(&ctx);
            let line = app.session.ids.mint();
            let root = app.session.doc.first_page();
            let mut ops = vec![Operation::CreateNode {
                id: line,
                parent: root,
                index: 1,
                kind: NodeKind::Line {
                    end: Point::new(60.0, 0.0),
                },
                transform: Some(Affine::translate((100.0, 300.0))),
                name: None,
            }];
            if locked {
                ops.push(Operation::SetLocked {
                    id: line,
                    locked: true,
                });
            }
            assert!(app.session.commit(Transaction(ops)));
            app.session.selection.set_one(line);
            frame(&ctx, &mut app, Vec::new(), Default::default());
            let depth = app.session.history.undo_depth();
            drag(
                &ctx,
                &mut app,
                Point::new(160.0, 300.0),
                Point::new(200.0, 330.0),
                none,
                false,
            );
            let moved = app.session.history.undo_depth() - depth;
            match locked {
                false => assert_eq!(moved, 1, "the fixture: the end drags unlocked"),
                true => assert_eq!(moved, 0, "the locked end"),
            }
        }
    }

    /// Whether, with nothing selected, the pointer on world `at` offers a band.
    fn hover_offers(ctx: &egui::Context, app: &mut OndinApp, at: Point) -> bool {
        app.session.selection.set(Vec::new());
        frame(ctx, app, Vec::new(), Default::default());
        let screen = app.to_screen(at, RECT, 1.0);
        app.spacing_at(screen, RECT, 1.0).is_some()
    }

    /// **A hovered container offers its bands only where a press would take it**
    /// (D1089): not inside a group, not as an instance, not locked — each a
    /// press the group, the instance or nothing takes. The control, the row
    /// alone, offers the gap.
    ///
    /// flips: `spacing_hover`'s `group_chain` or `instance_root` term removed →
    /// fails at *"grouped"* or *"an instance"*. Its `is_effectively_locked` term
    /// removed stays green: `hit_test` already passes over a locked layer, so
    /// that term is a second defence.
    #[test]
    fn a_hovered_container_in_a_group_an_instance_or_locked_offers_no_band() {
        let ctx = egui::Context::default();
        let gap = Point::new(65.0, 35.0);
        let (mut app, _) = app_with_a_row(&ctx);
        assert!(
            hover_offers(&ctx, &mut app, gap),
            "the control offers the gap"
        );

        let (mut app, row) = app_with_a_row(&ctx);
        let group = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::Reparent {
                id: row,
                new_parent: group,
                index: 0,
            },
        ])));
        assert!(!hover_offers(&ctx, &mut app, gap), "grouped");

        let (mut app, row) = app_with_a_row(&ctx);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetComponent {
                    id: row,
                    component: true,
                }]))
        );
        let place = ondin_core::Placement {
            nodes: app.session.doc.capture_subtree(row).unwrap(),
            parent: app.session.doc.first_page(),
            index: None,
        };
        let (mut tx, made) = ondin_core::insert_subtrees(
            &app.session.doc,
            &mut app.session.ids,
            &[place],
            Default::default(),
        );
        tx.0.push(Operation::SetTransform {
            id: made[0],
            transform: Affine::translate((0.0, 300.0)),
        });
        assert!(app.session.commit(tx));
        assert!(
            hover_offers(&ctx, &mut app, gap),
            "the fixture: the main still offers"
        );
        assert!(
            !hover_offers(&ctx, &mut app, Point::new(65.0, 335.0)),
            "an instance"
        );

        let (mut app, row) = app_with_a_row(&ctx);
        assert!(app.session.commit(Transaction(vec![Operation::SetLocked {
            id: row,
            locked: true,
        }])));
        assert!(!hover_offers(&ctx, &mut app, gap), "locked");
    }

    /// **A grid item spanning a gap takes a press on it**: the first item, 210
    /// wide across both columns, lies over the column gap at x 120–130, and a
    /// drag there selects and moves the item rather than writing the gap.
    ///
    /// flip: `hit`'s spanning-item yield removed → fails *"the gap stays"*, 18.
    #[test]
    fn a_grid_item_spanning_a_gap_takes_the_press() {
        use ondin_core::container::{
            Grid, GridLines, GridPlacement, Track, TrackBreadth as B, TrackSize as S,
        };
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let px = |v| Track::Size(S::Breadth(B::Px(v)));
        restyle(&mut app, row, |d| {
            *d = Display::Grid(Grid {
                columns: vec![px(100.0), px(100.0)],
                rows: vec![px(50.0), px(50.0)],
                column_gap: 10.0,
                row_gap: 6.0,
                padding: [20.0; 4],
                ..Default::default()
            });
        });
        let first = app.session.doc.get(row).unwrap().children()[0];
        let mut item = *app.session.doc.get(first).unwrap().item();
        item.grid_column = GridLines {
            start: GridPlacement::Auto,
            end: GridPlacement::Span(2),
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::SetLayoutItem { id: first, item },
            Operation::SetGeometry {
                id: first,
                geometry: ondin_core::GeometryPatch::Size(Size::new(210.0, 30.0)),
            },
        ])));
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let laid = app.session.resolved.world_bounds(first).unwrap();
        assert!(
            laid.x0 < 120.0 && laid.x1 > 130.0,
            "the fixture: the item spans the gap, {laid:?}"
        );
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(125.0, 35.0),
            Point::new(141.0, 35.0),
            none,
            false,
        );
        assert_eq!(spacing(&app, row)[0], 10.0, "the gap stays");
    }

    /// **A layout-styled container's drag detaches the style and says so**
    /// (§15 D1075): the row made the user of a layout style, its gap
    /// dragged to 18 — the style cut, the gap written, the floating status
    /// naming the style.
    #[test]
    fn a_layout_styled_containers_drag_detaches_the_style() {
        use ondin_core::style::{self, StyleSlot};
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let s = app.session.ids.mint_style();
        let ops = style::create_ops(
            &app.session.doc,
            s,
            "Stack/Row",
            row,
            StyleSlot::Layout,
            &[row],
        )
        .expect("the row has a layout");
        assert!(app.session.commit(Transaction(ops)));
        let styled = |app: &OndinApp| app.session.doc.get(row).unwrap().styles().layout;
        assert_eq!(styled(&app), Some(s), "the fixture: styled");
        app.session.info("");
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            false,
        );
        assert_eq!(styled(&app), None, "the drag cut the style");
        assert_eq!(spacing(&app, row)[0], 18.0);
        assert_eq!(app.session.status().text, "Detached Stack/Row");
    }

    /// **An instance's drag is an override**: the row made a main and an
    /// instance placed over it, the instance selected and its gap dragged to
    /// 18 — the copy at 18 and still linked, the main at 10.
    #[test]
    fn an_instances_drag_is_an_override() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetComponent {
                    id: row,
                    component: true,
                }]))
        );
        let doc = &app.session.doc;
        let (tx, made) = ondin_core::insert_subtrees(
            doc,
            &mut app.session.ids,
            &[ondin_core::Placement {
                nodes: doc.capture_subtree(row).unwrap(),
                parent: doc.first_page(),
                index: None,
            }],
            Default::default(),
        );
        assert!(app.session.commit(tx));
        let inst = made[0];
        assert_eq!(
            app.session.doc.get(inst).unwrap().link(),
            Some(row),
            "an instance"
        );
        app.session.selection.set_one(inst);
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            false,
        );
        assert_eq!(spacing(&app, inst)[0], 18.0, "the copy took the drag");
        assert_eq!(spacing(&app, row)[0], 10.0, "the main did not");
        assert_eq!(
            app.session.doc.get(inst).unwrap().link(),
            Some(row),
            "still linked"
        );
    }

    /// **A mask across the container does not hide its bands**: a mask draws
    /// nothing of its own, so a 400 × 200 mask over the whole row leaves the
    /// first gap and the top padding picking.
    ///
    /// Flip, run: the mask kept in the children list fails *"and the padding"* —
    /// a padding band yields to any child. Pinned is authored insets, not *"not a
    /// flex item"*, so the gap assertion alone does not see the mask.
    #[test]
    fn a_mask_across_the_container_does_not_hide_its_bands() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let mask = app.session.ids.mint();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: mask,
                parent: row,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(400.0, 200.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetMask {
                id: mask,
                mask: true
            },
        ])));
        app.session.selection.set_one(row);
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let picks = |app: &OndinApp, w: Point| {
            app.spacing_at(app.to_screen(w, RECT, 1.0), RECT, 1.0)
                .map(|(s, i)| s.bands[i].kind)
        };
        assert_eq!(
            picks(&app, Point::new(65.0, 35.0)),
            Some(super::BandKind::ColumnGap),
            "the gap under a mask still picks"
        );
        assert_eq!(
            picks(&app, Point::new(200.0, 14.0)),
            Some(super::BandKind::Padding(super::Side::Top)),
            "and the padding"
        );
    }

    /// **A hugging container grows with the drag**: the row's width fit to
    /// its content, a gap dragged +8 mid-drag (16 points at half rate) lays the
    /// row 16 wider — both gaps
    /// move. The size badge reads this same preview box; its drawing is not
    /// asserted here.
    #[test]
    fn a_hugging_container_grows_with_the_drag() {
        use ondin_core::container::Dimension;
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let mut item = *app.session.doc.get(row).unwrap().item();
        item.width = Dimension::FitContent;
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetLayoutItem {
                    id: row,
                    item
                }]))
        );
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let width = |app: &OndinApp| app.session.preview_local_box(row).unwrap().width();
        assert_eq!(
            width(&app),
            180.0,
            "the fixture hugs: 20 + 40 + 10 + 60 + 10 + 20 + 20"
        );
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(65.0, 35.0),
            Point::new(81.0, 35.0),
            none,
            true,
        );
        assert_eq!(width(&app), 196.0, "the preview grows by both gaps");
    }

    /// **A hugging container its parent aligns to the end drags at half rate
    /// and stands still with the hand**: the row, hugging, the only item of a
    /// 600-wide row justified to the end, its gap dragged 16 — 18, previewed and
    /// committed, though each frame's widening moves its origin left.
    ///
    /// flip: `band_drag_tx` measuring in `preview_world_transform` → fails
    /// *"half rate in an end-aligned parent"*, 48.
    #[test]
    fn an_end_aligned_hugging_container_drags_at_half_rate() {
        use ondin_core::container::{Dimension, JustifyContent};
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let outer = app.session.ids.mint();
        let root = app.session.doc.first_page();
        let mut item = *app.session.doc.get(row).unwrap().item();
        item.width = Dimension::FitContent;
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: outer,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(600.0, 200.0),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::SetDisplay {
                id: outer,
                display: Some(Display::Flex(Flex {
                    justify_content: JustifyContent::End,
                    align_items: AlignItems::Start,
                    ..Default::default()
                })),
            },
            Operation::Reparent {
                id: row,
                new_parent: outer,
                index: 0,
            },
            Operation::SetLayoutItem { id: row, item },
        ])));
        app.session.camera.center = Point::new(400.0, 100.0);
        app.session.selection.set_one(row);
        for _ in 0..2 {
            frame(&ctx, &mut app, Vec::new(), Default::default());
        }
        let x0 = app
            .session
            .resolved
            .world_transform(row)
            .unwrap()
            .translation()
            .x;
        assert_eq!(x0, 420.0, "the fixture: 180 wide at the end of 600");
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(x0 + 65.0, 35.0),
            Point::new(x0 + 81.0, 35.0),
            none,
            false,
        );
        assert_eq!(
            spacing(&app, row)[0],
            18.0,
            "half rate in an end-aligned parent"
        );
    }

    /// **A rotated container's bands pick where they are drawn**: the row
    /// turned a quarter about its origin, the first gap's middle at world
    /// (−35, 65) picks the gap with the vertical arrows, and a drag 16 down (8
    /// at half rate) — along the row's own x — is a gap of 18.
    ///
    /// Flip, run: `spacing_at` testing the world point unmapped by the row's
    /// inverse transform fails on *"the gap, turned"*. The layout-style, instance
    /// and hugging row's tests are not flipped: each rests on a pass with its own tests
    /// (`style::settle`, the override marks, the flex engine's fit-content).
    #[test]
    fn a_rotated_containers_bands_pick_where_drawn() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: row,
                    transform: Affine::rotate(std::f64::consts::FRAC_PI_2),
                }]))
        );
        frame(&ctx, &mut app, Vec::new(), Default::default());
        let at = app.to_screen(Point::new(-35.0, 65.0), RECT, 1.0);
        let (s, i) = app.spacing_at(at, RECT, 1.0).expect("the gap, turned");
        assert_eq!(s.bands[i].kind, super::BandKind::ColumnGap);
        assert_eq!(
            app.spacing_cursor(row, s.bands[i].kind),
            egui::CursorIcon::ResizeVertical
        );
        let none = Default::default();
        drag(
            &ctx,
            &mut app,
            Point::new(-35.0, 65.0),
            Point::new(-35.0, 81.0),
            none,
            false,
        );
        assert_eq!(spacing(&app, row)[0], 18.0);
    }

    /// **The cursor says resize only over a band**: across a gap the
    /// horizontal arrows, across the top padding the vertical, over an item the
    /// plain arrow, and with two layers selected nothing over the same gap.
    #[test]
    fn the_cursor_changes_only_over_a_band() {
        let ctx = egui::Context::default();
        let (mut app, row) = app_with_a_row(&ctx);
        let over = |app: &mut OndinApp, w: Point| {
            let p = app.to_screen(w, RECT, 1.0);
            frame(
                &ctx,
                app,
                vec![egui::Event::PointerMoved(p)],
                Default::default(),
            );
            frame(&ctx, app, Vec::new(), Default::default())
                .platform_output
                .cursor_icon
        };
        assert_eq!(
            over(&mut app, Point::new(65.0, 35.0)),
            egui::CursorIcon::ResizeHorizontal
        );
        assert_eq!(
            over(&mut app, Point::new(200.0, 14.0)),
            egui::CursorIcon::ResizeVertical
        );
        assert_ne!(
            over(&mut app, Point::new(40.0, 35.0)),
            egui::CursorIcon::ResizeHorizontal
        );
        let item = app.session.doc.get(row).unwrap().children()[0];
        app.session.selection.set(vec![row, item]);
        assert_ne!(
            over(&mut app, Point::new(65.0, 35.0)),
            egui::CursorIcon::ResizeHorizontal,
            "several selected: no bands"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::container::{Flex, FlexDirection, FlexWrap};

    fn row(gap: f64, pad: f64) -> Display {
        Display::Flex(Flex {
            column_gap: gap,
            row_gap: gap,
            padding: [pad; 4],
            ..Flex::default()
        })
    }

    /// **A row's bands**: four paddings edge to edge, and a between-items gap
    /// between each pair of neighbours across the line's cross band — a 300×100
    /// row padded 20 with three 60-wide items at gap 10.
    #[test]
    fn a_rows_bands_are_its_padding_and_its_gaps() {
        let bx = Rect::new(0.0, 0.0, 300.0, 100.0);
        let items = [
            Rect::new(20.0, 20.0, 80.0, 80.0),
            Rect::new(90.0, 20.0, 150.0, 60.0),
            Rect::new(160.0, 20.0, 220.0, 80.0),
        ];
        let b = bands(&row(10.0, 20.0), bx, &items, None);
        let gaps: Vec<&Band> = b.iter().filter(|b| b.kind == BandKind::ColumnGap).collect();
        assert_eq!(gaps.len(), 2);
        assert_eq!(
            gaps[0].rect,
            Rect::new(80.0, 20.0, 90.0, 80.0),
            "across the line's band"
        );
        assert_eq!(gaps[0].name, "Gap");
        let top = b
            .iter()
            .find(|b| b.kind == BandKind::Padding(Side::Top))
            .unwrap();
        assert_eq!(top.rect, Rect::new(0.0, 0.0, 300.0, 20.0), "edge to edge");
        assert!(
            b.iter().all(|b| b.kind != BandKind::RowGap),
            "one line: no line gap"
        );
    }

    /// **A column's between-items gap is a `RowGap`** — the vertical gap, as the
    /// engine reads it — and a wrapped row's line gap runs across the content's
    /// whole width, under a short last line too.
    #[test]
    fn a_columns_gap_is_vertical_and_a_wrapped_rows_line_gap_spans_the_content() {
        let col = Display::Flex(Flex {
            direction: FlexDirection::Column,
            row_gap: 10.0,
            ..Flex::default()
        });
        let items = [
            Rect::new(0.0, 0.0, 50.0, 40.0),
            Rect::new(0.0, 50.0, 50.0, 90.0),
        ];
        let b = bands(&col, Rect::new(0.0, 0.0, 50.0, 90.0), &items, None);
        let gap = b
            .iter()
            .find(|b| b.kind == BandKind::RowGap)
            .expect("a vertical gap");
        assert_eq!(gap.rect, Rect::new(0.0, 40.0, 50.0, 50.0));
        assert_eq!(gap.name, "Gap");

        let wrapped = Display::Flex(Flex {
            wrap: FlexWrap::Wrap,
            column_gap: 10.0,
            row_gap: 8.0,
            ..Flex::default()
        });
        let items = [
            Rect::new(0.0, 0.0, 60.0, 30.0),
            Rect::new(70.0, 0.0, 130.0, 30.0),
            Rect::new(0.0, 38.0, 60.0, 68.0),
        ];
        let b = bands(&wrapped, Rect::new(0.0, 0.0, 140.0, 68.0), &items, None);
        let line = b
            .iter()
            .find(|b| b.kind == BandKind::RowGap)
            .expect("a line gap");
        assert_eq!(
            line.rect,
            Rect::new(0.0, 30.0, 140.0, 38.0),
            "the whole content width"
        );
        assert_eq!(line.name, "Row gap");
        assert_eq!(
            b.iter().filter(|b| b.kind == BandKind::ColumnGap).count(),
            1,
            "per line"
        );
    }

    /// **A grid's bands come from its laid tracks**: a column-gap band
    /// between each pair of columns across the rows' whole run, a row-gap band
    /// likewise, each writing its own axis.
    #[test]
    fn a_grids_bands_are_between_its_tracks() {
        use ondin_core::container::{Grid, LaidTracks};
        let tracks = |spans: Vec<(f64, f64)>| LaidTracks {
            spans,
            before: 0,
            explicit: 2,
        };
        let grid = LaidGrid {
            columns: tracks(vec![(0.0, 50.0), (60.0, 110.0)]),
            rows: tracks(vec![(0.0, 30.0), (38.0, 68.0)]),
            areas: Vec::new(),
        };
        let display = Display::Grid(Grid {
            column_gap: 10.0,
            row_gap: 8.0,
            ..Grid::default()
        });
        let b = bands(&display, Rect::new(0.0, 0.0, 110.0, 68.0), &[], Some(&grid));
        let col = b
            .iter()
            .find(|b| b.kind == BandKind::ColumnGap)
            .expect("a column gap");
        assert_eq!(col.rect, Rect::new(50.0, 0.0, 60.0, 68.0));
        assert_eq!((col.stored, col.name), (10.0, "Column gap"));
        let row = b
            .iter()
            .find(|b| b.kind == BandKind::RowGap)
            .expect("a row gap");
        assert_eq!(row.rect, Rect::new(0.0, 30.0, 110.0, 38.0));
        assert_eq!((row.stored, row.name), (8.0, "Row gap"));
    }

    /// **Picking**: a corner is the top padding's, a zero gap picks
    /// across a strip centred on its seam, nothing picks within the resize
    /// radius of the edge, and a thin padding is still reachable past it. A
    /// pinned child takes its own click over a padding band and over a gap band
    /// alike (§15 D1078's second amendment); an in-flow neighbour does not stop
    /// a gap's strip.
    ///
    /// Flip, run: a gap band yielding to no child (its old rule) fails *"a
    /// pinned child over a gap is the child's"*.
    #[test]
    fn picking_follows_the_priority_and_spares_the_resize_edge() {
        let bx = Rect::new(0.0, 0.0, 300.0, 100.0);
        let items = [
            Rect::new(20.0, 20.0, 80.0, 80.0),
            Rect::new(80.0, 20.0, 140.0, 80.0),
        ];
        let b = bands(&row(0.0, 20.0), bx, &items, None);
        let edge = Vec2::new(9.0, 9.0);
        let min = Vec2::new(8.0, 8.0);
        let in_flow: Vec<(Rect, bool)> = items.iter().map(|r| (*r, false)).collect();
        let at = |x, y| hit(&b, Point::new(x, y), bx, &in_flow, min, edge).map(|i| b[i].kind);
        assert_eq!(
            at(15.0, 15.0),
            Some(BandKind::Padding(Side::Top)),
            "the corner is the top's"
        );
        assert_eq!(
            at(81.0, 50.0),
            Some(BandKind::ColumnGap),
            "a zero gap still picks"
        );
        assert_eq!(at(70.0, 50.0), None, "an item is not a band");
        assert_eq!(at(150.0, 4.0), None, "the resize edge wins");
        // A pinned child sitting on the top padding, and one across the zero
        // gap's strip, each take their own click.
        let mut pinned = in_flow.clone();
        pinned.push((Rect::new(150.0, 10.0, 170.0, 18.0), true));
        pinned.push((Rect::new(78.0, 40.0, 84.0, 60.0), true));
        assert_eq!(
            hit(&b, Point::new(160.0, 14.0), bx, &pinned, min, edge),
            None,
            "a child on the padding is the child's"
        );
        assert_eq!(
            hit(&b, Point::new(81.0, 50.0), bx, &pinned, min, edge),
            None,
            "a pinned child over a gap is the child's"
        );
        // Padding 4: thinner than the resize radius, still grabbed past it.
        let thin = bands(&row(10.0, 4.0), bx, &[], None);
        let i = hit(&thin, Point::new(150.0, 12.0), bx, &[], min, edge);
        assert_eq!(i.map(|i| thin[i].kind), Some(BandKind::Padding(Side::Top)));
    }

    /// **The write**: a delta at half the pointer's travel (§15
    /// D1094) in whole units floored at 0, a right padding growing as the
    /// pointer moves in, `Shift` stepping by 10, and the modifiers choosing the
    /// sides; a bound slot left untouched keeps its literal.
    #[test]
    fn a_drag_writes_whole_units_on_the_sides_its_modifiers_name() {
        let right = BandKind::Padding(Side::Right);
        assert_eq!(
            dragged_value(right, 16.0, Vec2::new(-10.8, 0.0), false),
            21.0,
            "10.8 in is 5.4 at half rate, 21 in whole units"
        );
        assert_eq!(
            dragged_value(BandKind::ColumnGap, 16.0, Vec2::new(-40.0, 0.0), false),
            0.0
        );
        assert_eq!(
            dragged_value(BandKind::RowGap, 16.0, Vec2::new(0.0, 13.0), true),
            26.0
        );
        assert_eq!(written_slots(right, false, false), vec![3]);
        assert_eq!(written_slots(right, true, false), vec![3, 5]);
        assert_eq!(written_slots(right, true, true), vec![2, 3, 4, 5]);
        assert_eq!(written_slots(BandKind::ColumnGap, true, true), vec![0]);
        let d = written(&row(10.0, 20.0), &[3, 5], 28.0);
        assert_eq!(d.padding(), [20.0, 28.0, 20.0, 28.0]);
        assert_eq!(d.gaps(), (10.0, 10.0));
    }

    /// **The label**: the sides it writes, and under derived space the
    /// laid number beside the stored one.
    #[test]
    fn the_label_names_the_sides_and_the_laid_space() {
        let pad = Band {
            kind: BandKind::Padding(Side::Left),
            rect: Rect::ZERO,
            stored: 16.0,
            room: 100.0,
            name: "Padding",
        };
        assert_eq!(label(&pad, 28.0, &[5, 3], None), "Padding 28 · L R");
        // Grabbed on the right or the bottom, still in reading order.
        assert_eq!(label(&pad, 28.0, &[3, 5], None), "Padding 28 · L R");
        assert_eq!(label(&pad, 28.0, &[4, 2], None), "Padding 28 · T B");
        assert_eq!(label(&pad, 28.0, &[2, 3, 4, 5], None), "Padding 28 · all");
        let gap = Band {
            kind: BandKind::ColumnGap,
            name: "Gap",
            ..pad
        };
        assert_eq!(label(&gap, 8.0, &[0], Some(52.0)), "Gap 8 · laid 52");
        assert_eq!(label(&gap, 24.0, &[0], Some(24.0)), "Gap 24");
    }

    /// **A band too thin on screen is dropped**: at a quarter scale a 16-unit band is 4 points and dropped, and a
    /// zero gap goes with it once its strip would crowd its neighbours.
    #[test]
    fn a_band_too_thin_on_screen_is_dropped() {
        let bx = Rect::new(0.0, 0.0, 300.0, 100.0);
        let items = [
            Rect::new(16.0, 16.0, 76.0, 84.0),
            Rect::new(76.0, 16.0, 136.0, 84.0),
        ];
        let all = bands(&row(0.0, 16.0), bx, &items, None);
        assert_eq!(shown(all.clone(), Vec2::new(1.0, 1.0)).len(), 5);
        assert!(shown(all, Vec2::new(0.25, 0.25)).is_empty());
    }
}
