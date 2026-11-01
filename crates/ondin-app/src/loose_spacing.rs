//! Spacing handles on a loose selection: the container's gap bands brought to
//! layers in a row, a column or a grid with no container, evenly spaced.
//!
//! The arrangement is core's (`build::arrangement`): *Tidy up*'s rows and columns,
//! each layer in a cell of its own, the gaps of each axis even to half a unit.
//! Dragging a band sets every gap of its axis (`build::respaced`) at the
//! containers' rate, the key layer's band holding still, else the first layer's in
//! reading order. A layer's centre dot drags it through the run's slots, the
//! others sliding over (`build::reordered`); the layer tree is left alone.
//!
//! Shown under Select or Scale for two or more unlocked, unrotated, shown layers
//! of one parent that is not a flex or grid container, whose own bands rule there.

use crate::app::OndinApp;
use crate::preview::Drag;
use crate::spacing::{BandKind, MIN_BAND_PX};
use crate::theme::{color, icon};
use eframe::egui;
use ondin_core::build::{self, Arrangement, Axis};
use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_core::{NodeId, Transaction};

/// The centre dot's radius, and how near it a press must land, in screen points:
/// the shape handles' dot and reach, so every such dot matches.
const DOT_R: f32 = 3.5;
const DOT_PICK_PX: f32 = 8.0;

/// A loose selection read as a run: its layers, their world boxes in the same
/// order, and the arrangement they form.
pub(crate) struct Run {
    pub ids: Vec<NodeId>,
    pub boxes: Vec<Rect>,
    pub arr: Arrangement,
}

impl Run {
    /// The member a drag holds still: the key layer, else the first in reading
    /// order.
    fn anchor(&self, key: Option<NodeId>) -> usize {
        key.and_then(|k| self.ids.iter().position(|i| *i == k))
            .or_else(|| self.arr.reading_order().first().copied())
            .unwrap_or(0)
    }

    /// The gap bands in world space, with their axis: between neighbouring
    /// columns across each row, and between neighbouring rows across each column,
    /// where a cell either side is filled.
    pub(crate) fn bands(&self) -> Vec<(Axis, Rect)> {
        let filled = |r: usize, c: usize| self.arr.cells.contains(&(r, c));
        let (cols, rows) = (&self.arr.cols, &self.arr.rows);
        let mut out = Vec::new();
        for k in 1..cols.len() {
            for (r, row) in rows.iter().enumerate() {
                if filled(r, k - 1) || filled(r, k) {
                    out.push((Axis::X, Rect::new(cols[k - 1].1, row.0, cols[k].0, row.1)));
                }
            }
        }
        for k in 1..rows.len() {
            for (c, col) in cols.iter().enumerate() {
                if filled(k - 1, c) || filled(k, c) {
                    out.push((Axis::Y, Rect::new(col.0, rows[k - 1].1, col.1, rows[k].0)));
                }
            }
        }
        out
    }
}

/// What a press on a run grabs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Grab {
    /// The gaps of one axis.
    Gap(Axis),
    /// A member's centre dot, by index.
    Dot(usize),
}

/// The container band kind whose drag rate and cursor an axis's gap shares.
fn kind(axis: Axis) -> BandKind {
    match axis {
        Axis::X => BandKind::ColumnGap,
        Axis::Y => BandKind::RowGap,
    }
}

impl OndinApp {
    /// The selection as a run, its boxes read through `bounds`, or `None` where it
    /// has no handles ([module docs](self)).
    fn run_with(&self, bounds: impl Fn(NodeId) -> Option<Rect>) -> Option<Run> {
        if !self.selecting() || self.editing_in_place() {
            return None;
        }
        let doc = &self.session.doc;
        let ids = self.session.selection.ids().to_vec();
        if ids.len() < 2 {
            return None;
        }
        let parent = doc.get(ids[0])?.parent()?;
        // A flex or grid container's own bands rule among its children.
        if doc.resolved_display(parent).is_some() {
            return None;
        }
        let mut boxes = Vec::with_capacity(ids.len());
        for id in &ids {
            let node = doc.get(*id)?;
            if node.parent() != Some(parent)
                || ondin_core::is_effectively_locked(doc, *id)
                || !self.shown_visible(*id)
            {
                return None;
            }
            // Unrotated: a turned layer's world bounds are not its box.
            let c = self.session.resolved.world_transform(*id)?.as_coeffs();
            if c[1].abs() > 1e-9 || c[2].abs() > 1e-9 {
                return None;
            }
            boxes.push(bounds(*id)?);
        }
        let arr = build::arrangement(&boxes)?;
        // Two layers always read as evenly spaced; only side by side are they a run,
        // not two corners of a grid whose bands lie over open canvas.
        if ids.len() == 2 && arr.rows.len() > 1 && arr.cols.len() > 1 {
            return None;
        }
        Some(Run { ids, boxes, arr })
    }

    /// The run as committed: what a drag measures from.
    pub(crate) fn loose_run(&self) -> Option<Run> {
        self.run_with(|id| self.session.resolved.world_bounds(id))
    }

    /// The run as drawn, a live drag's moves included.
    fn shown_run(&self) -> Option<Run> {
        self.run_with(|id| self.session.preview_world_bounds(id))
    }

    /// Whether `axis`'s gap is too thin on screen for its bands.
    fn thin(&self, run: &Run, axis: Axis) -> bool {
        run.arr
            .bands(axis)
            .1
            .is_none_or(|g| g * self.points_per_world() < MIN_BAND_PX)
    }

    /// What a press at `screen` would grab on the run: a hovered member's centre
    /// dot first, then a gap band. Only at rest.
    pub(crate) fn loose_at(&self, screen: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<Grab> {
        if !self.drag.is_none() {
            return None;
        }
        let run = self.loose_run()?;
        let world = self.to_world(screen, rect, ppp);
        // Alt drags a copy, and a member too small on screen is all dot: both move.
        let alt = self.egui_ctx.input(|i| i.modifiers.alt);
        let ppw = self.points_per_world();
        for (i, b) in run.boxes.iter().enumerate() {
            let centre = self.to_screen(b.center(), rect, ppp);
            let big = b.width().min(b.height()) * ppw >= crate::shape_handles::MIN_SIDE_PX;
            if !alt && big && b.contains(world) && centre.distance(screen) <= DOT_PICK_PX {
                return Some(Grab::Dot(i));
            }
        }
        // A loose gap is open canvas: a layer outside the run lying in it takes the
        // press, as it would with no selection.
        let doc = &self.session.doc;
        let outside = self.pick_leaf(world).is_some_and(|hit| {
            !std::iter::successors(Some(hit), |n| doc.get(*n).and_then(|n| n.parent()))
                .any(|a| run.ids.contains(&a))
        });
        if outside {
            return None;
        }
        run.bands()
            .into_iter()
            .find(|(axis, r)| !self.thin(&run, *axis) && r.contains(world))
            .map(|(axis, _)| Grab::Gap(axis))
    }

    /// Start a run drag at the press `grab`, or `false` where it grabs nothing.
    pub(crate) fn begin_loose_drag(
        &mut self,
        grab: egui::Pos2,
        rect: egui::Rect,
        ppp: f32,
    ) -> bool {
        let Some(what) = self.loose_at(grab, rect, ppp) else {
            return false;
        };
        let Some(run) = self.loose_run() else {
            return false;
        };
        let press = self.to_world(grab, rect, ppp);
        self.drag = match what {
            Grab::Gap(axis) => Drag::LooseGap {
                axis,
                press,
                base: run.arr.bands(axis).1.unwrap_or(0.0),
            },
            Grab::Dot(i) => Drag::LooseReorder {
                id: run.ids[i],
                press,
            },
        };
        true
    }

    /// The transaction a run drag at world `world` previews and commits, with its
    /// undo label; `None` when the selection no longer reads as a run.
    pub(crate) fn loose_drag_tx(&self, world: Point, shift: bool) -> Option<(Transaction, String)> {
        let run = self.loose_run()?;
        let doc = &self.session.doc;
        let res = &self.session.resolved;
        let n = run.ids.len();
        match self.drag {
            Drag::LooseGap { axis, press, base } => {
                let value = crate::spacing::dragged_value(kind(axis), base, world - press, shift);
                let anchor = run.anchor(self.session.selection.key());
                let moves = build::respaced(&run.arr, axis, anchor, value);
                Some((
                    build::offsets(doc, res, &run.ids, &moves),
                    format!("Spacing · {n} layers"),
                ))
            }
            Drag::LooseReorder { id, press } => {
                let from = run.ids.iter().position(|i| *i == id)?;
                let to = self.loose_slot(&run, from, world - press);
                let moves = build::reordered(&run.boxes, &run.arr, from, to);
                Some((
                    build::offsets(doc, res, &run.ids, &moves),
                    format!("Reorder · {n} layers"),
                ))
            }
            _ => None,
        }
    }

    /// The slot of the reading order nearest member `from`'s centre moved by
    /// `delta`: where letting go would land it.
    fn loose_slot(&self, run: &Run, from: usize, delta: Vec2) -> usize {
        let at = run.boxes[from].center() + delta;
        let slots = run.arr.reading_order();
        (0..slots.len())
            .min_by(|a, b| {
                let d = |s: usize| (run.boxes[slots[s]].center() - at).hypot();
                d(*a).total_cmp(&d(*b))
            })
            .unwrap_or(0)
    }

    /// The release of a run drag: one step, or nothing where nothing moved.
    pub(crate) fn finish_loose_drag(&mut self, world: Point, shift: bool) {
        let built = self.loose_drag_tx(world, shift);
        self.drag = Drag::None;
        match built {
            Some((tx, label)) if !tx.0.is_empty() => {
                self.session
                    .name_step(label, icon::ARROWS_OUT_LINE_HORIZONTAL);
                self.session.commit(tx);
            }
            _ => self.session.clear_preview(),
        }
    }

    /// The cursor over a run's handle, or while dragging one.
    pub(crate) fn loose_cursor(&self, grab: Option<Grab>) -> Option<egui::CursorIcon> {
        let axis_icon = |axis: Axis| match axis {
            Axis::X => egui::CursorIcon::ResizeHorizontal,
            Axis::Y => egui::CursorIcon::ResizeVertical,
        };
        match (&self.drag, grab) {
            (Drag::LooseGap { axis, .. }, _) => Some(axis_icon(*axis)),
            (Drag::LooseReorder { .. }, _) => Some(egui::CursorIcon::Grabbing),
            (_, Some(Grab::Gap(axis))) => Some(axis_icon(axis)),
            (_, Some(Grab::Dot(_))) => Some(egui::CursorIcon::Grab),
            _ => None,
        }
    }

    /// The handles' drawing: nothing at rest; with the pointer over the selection,
    /// every gap band faint, the one under the pointer strong with its pill, and
    /// the hovered layer's centre dot. Dragging a gap, its axis's bands strong and
    /// the value beside the pointer; reordering, the layer as a ghost under the
    /// pointer while the run previews where it lands.
    pub(crate) fn draw_loose_spacing(&self, painter: &egui::Painter, rect: egui::Rect, ppp: f32) {
        if self.mode != crate::input::Mode::Normal || self.chrome_hidden() {
            return;
        }
        let dragging = matches!(self.drag, Drag::LooseGap { .. } | Drag::LooseReorder { .. });
        if !dragging && !self.drag.is_none() {
            return;
        }
        let Some(pointer) = painter.ctx().input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let world = self.to_world(pointer, rect, ppp);
        let screen = |r: Rect| {
            egui::Rect::from_two_pos(
                self.to_screen(Point::new(r.x0, r.y0), rect, ppp),
                self.to_screen(Point::new(r.x1, r.y1), rect, ppp),
            )
        };
        // Ahead of the run: a grid of mixed sizes, previewed mid-reorder, may not
        // read as one, and the ghost must still follow the pointer.
        if let Drag::LooseReorder { id, press } = self.drag
            && let Some(b) = self.session.resolved.world_bounds(id)
        {
            let ghost = screen(b + (world - press));
            painter.rect_filled(ghost, 4.0, color::SELECT.gamma_multiply(0.3));
            painter.rect_stroke(
                ghost,
                4.0,
                egui::Stroke::new(1.0, color::SELECT),
                egui::StrokeKind::Middle,
            );
            paint_dot(painter, ghost.center());
            return;
        }
        let Some(run) = self.shown_run() else {
            return;
        };
        let hue = crate::canvas::chrome_hue(&self.session.doc, run.ids[0]);
        match self.drag {
            Drag::LooseGap { axis, press, base } => {
                for (a, r) in run.bands() {
                    if a == axis {
                        painter.rect_filled(screen(r), 0.0, color::SPACING_STRONG);
                    }
                }
                let shift = painter.ctx().input(|i| i.modifiers.shift);
                let value = crate::spacing::dragged_value(kind(axis), base, world - press, shift);
                let n = crate::ui::number(0)(value, 0..=0);
                crate::spacing::paint_label(painter, pointer, &format!("Gap {n}"), None, hue);
                return;
            }
            Drag::LooseReorder { .. } => return,
            _ => {}
        }
        let union = run.boxes.iter().copied().reduce(|a, b| a.union(b));
        if !union.is_some_and(|u| u.contains(world)) {
            return;
        }
        let hovered = self.loose_at(pointer, rect, ppp);
        for (axis, r) in run.bands() {
            if self.thin(&run, axis) {
                continue;
            }
            let lit = hovered == Some(Grab::Gap(axis)) && r.contains(world);
            let fill = if lit {
                color::SPACING_STRONG
            } else {
                color::GRID_GAP
            };
            painter.rect_filled(screen(r), 0.0, fill);
            if lit {
                let s = screen(r);
                let quad = [
                    s.left_top(),
                    s.right_top(),
                    s.right_bottom(),
                    s.left_bottom(),
                ];
                crate::spacing::paint_pill(painter, quad, kind(axis), hue);
            }
        }
        if let Some(b) = run.boxes.iter().find(|b| b.contains(world)) {
            paint_dot(painter, self.to_screen(b.center(), rect, ppp));
        }
    }
}

/// A layer's centre dot: white, ringed in the selection blue.
fn paint_dot(painter: &egui::Painter, at: egui::Pos2) {
    painter.circle(
        at,
        DOT_R,
        egui::Color32::WHITE,
        egui::Stroke::new(1.5, color::SELECT),
    );
}

/// The handles driven through the whole canvas with real pointer events.
#[cfg(test)]
mod tests {
    use crate::app::OndinApp;
    use ondin_core::kurbo::{Affine, Point, Size};
    use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, Transaction};

    const RECT: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(800.0, 600.0),
    };

    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let time = ctx.input(|i| i.time) + 0.1;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(RECT),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| app.canvas_ui(ui),
        );
    }

    fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Press at world `from`, drag to `to` in eight steps, release.
    fn drag(ctx: &egui::Context, app: &mut OndinApp, from: Point, to: Point) {
        let at = |app: &OndinApp, w: Point| app.to_screen(w, RECT, 1.0);
        let start = at(app, from);
        frame(ctx, app, vec![egui::Event::PointerMoved(start)]);
        frame(ctx, app, vec![button(start, true)]);
        for step in 1..=8 {
            let p = at(app, from + (to - from) * (step as f64 / 8.0));
            frame(ctx, app, vec![egui::Event::PointerMoved(p)]);
        }
        frame(ctx, app, vec![button(at(app, to), false)]);
        frame(ctx, app, Vec::new());
    }

    /// A 600 × 300 frame at the origin holding four 78 × 80 cards at y 60, the
    /// first at x 40 and then `gaps` apart, all selected; camera on the frame's
    /// centre at 100%.
    fn app_with_cards(ctx: &egui::Context, gaps: [f64; 3]) -> (OndinApp, Vec<NodeId>) {
        crate::theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x5BAC);
        let root = ids.mint();
        let board = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(600.0, 300.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }];
        let mut cards = Vec::new();
        let mut x = 40.0;
        for i in 0..4 {
            let id = ids.mint();
            cards.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: board,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(78.0, 80.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((x, 60.0))),
                name: None,
            });
            x += 78.0 + gaps.get(i).copied().unwrap_or(0.0);
        }
        let mut doc = Document::new(root);
        doc.apply(&Transaction(ops)).expect("the cards");
        app.session.adopt_document(doc, None);
        app.session.camera.center = Point::new(300.0, 150.0);
        app.session.camera.zoom = 1.0;
        app.session.selection.set(cards.clone());
        for _ in 0..2 {
            frame(ctx, &mut app, Vec::new());
        }
        (app, cards)
    }

    fn xs(app: &OndinApp, cards: &[NodeId]) -> Vec<f64> {
        cards
            .iter()
            .map(|c| app.session.doc.get(*c).unwrap().transform().translation().x)
            .collect()
    }

    /// Dragging a gap 24 pt sets every gap to 28 in one *Spacing* step, the first
    /// card holding still; with a key layer, the key holds still instead.
    /// flip: `loose_drag_tx` ignoring the key layer → fails at "the key holds".
    #[test]
    fn dragging_a_gap_sets_every_gap_around_the_held_card() {
        let ctx = egui::Context::default();
        let (mut app, cards) = app_with_cards(&ctx, [16.0; 3]);
        drag(
            &ctx,
            &mut app,
            Point::new(220.0, 100.0),
            Point::new(244.0, 100.0),
        );
        assert_eq!(xs(&app, &cards), vec![40.0, 146.0, 252.0, 358.0]);
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Spacing · 4 layers");
        assert_eq!(app.session.selection.ids().len(), 4, "the selection stays");

        app.session.selection.set_key(cards[2]);
        // The third gap now runs x 330–358; 24 pt back takes every gap to 16.
        drag(
            &ctx,
            &mut app,
            Point::new(344.0, 100.0),
            Point::new(320.0, 100.0),
        );
        assert_eq!(xs(&app, &cards)[2], 252.0, "the key holds");
        assert_eq!(xs(&app, &cards), vec![64.0, 158.0, 252.0, 346.0]);
    }

    /// The first card's centre dot dragged past the second card's middle swaps the
    /// two, one *Reorder* step, and the layer tree keeps its order.
    /// flip: `begin_loose_drag` without its `Dot` arm → fails at the positions.
    #[test]
    fn a_centre_dot_drags_a_card_through_the_run() {
        let ctx = egui::Context::default();
        let (mut app, cards) = app_with_cards(&ctx, [16.0; 3]);
        let board = app.session.doc.get(cards[0]).unwrap().parent().unwrap();
        let before = app.session.doc.get(board).unwrap().children().to_vec();
        drag(
            &ctx,
            &mut app,
            Point::new(79.0, 100.0),
            Point::new(180.0, 100.0),
        );
        assert_eq!(xs(&app, &cards), vec![134.0, 40.0, 228.0, 322.0]);
        let step = app.session.history.done().last().expect("a step");
        assert_eq!(step.note.label, "Reorder · 4 layers");
        assert_eq!(app.session.doc.get(board).unwrap().children(), &before[..]);
    }

    /// Uneven gaps or one layer offer no run, and a press in an uneven gap is the
    /// marquee's.
    #[test]
    fn uneven_gaps_or_one_layer_offer_no_handles() {
        let ctx = egui::Context::default();
        let (mut app, cards) = app_with_cards(&ctx, [10.0, 26.0, 14.0]);
        assert!(app.loose_run().is_none(), "uneven");
        let (mut even, _) = app_with_cards(&ctx, [16.0; 3]);
        assert!(even.loose_run().is_some());
        even.session.selection.set_one(cards[0]);
        assert!(even.loose_run().is_none(), "one layer");
        // A press in the middle of an uneven gap (x 206–232) is the marquee's.
        drag(
            &ctx,
            &mut app,
            Point::new(219.0, 100.0),
            Point::new(230.0, 120.0),
        );
        assert_eq!(xs(&app, &cards), vec![40.0, 128.0, 232.0, 324.0]);
    }

    /// A press on an unselected layer lying in a loose gap picks and moves that
    /// layer; the band yields to it.
    /// flip: `loose_at` without its `outside` yield → fails at "the card under the
    /// press moved".
    #[test]
    fn a_layer_in_a_loose_gap_takes_the_press() {
        let ctx = egui::Context::default();
        let (mut app, cards) = app_with_cards(&ctx, [16.0; 3]);
        app.session.selection.set(vec![cards[0], cards[3]]);
        frame(&ctx, &mut app, Vec::new());
        assert!(
            app.loose_run().is_some(),
            "fixture: two cards in a row are a run"
        );
        // The second card's middle, inside the band between the outer two.
        drag(
            &ctx,
            &mut app,
            Point::new(173.0, 80.0),
            Point::new(193.0, 80.0),
        );
        let after = xs(&app, &cards);
        assert!(
            after[1] > 134.0,
            "the card under the press moved: {after:?}"
        );
        assert_eq!(
            [after[0], after[2], after[3]],
            [40.0, 228.0, 322.0],
            "the run did not"
        );
        assert_eq!(app.session.selection.ids(), [cards[1]]);
    }

    /// Zoomed out until the cards are small on screen, a press on a card's centre
    /// moves the selection rather than reordering it.
    /// flip: `loose_at` without its `big` test → fails at the positions.
    #[test]
    fn a_small_member_has_no_centre_dot() {
        let ctx = egui::Context::default();
        let (mut app, cards) = app_with_cards(&ctx, [16.0; 3]);
        app.session.camera.zoom = 0.25;
        frame(&ctx, &mut app, Vec::new());
        drag(
            &ctx,
            &mut app,
            Point::new(79.0, 100.0),
            Point::new(179.0, 100.0),
        );
        let moved: Vec<f64> = xs(&app, &cards)
            .iter()
            .zip([40.0, 134.0, 228.0, 322.0])
            .map(|(x, was)| x - was)
            .collect();
        assert!(
            moved[0] > 0.0 && moved.iter().all(|m| *m == moved[0]),
            "{moved:?}"
        );
    }

    /// A flex parent, a locked member, a turned member and two diagonal layers each
    /// offer no run.
    /// flip: `run_with` without its `resolved_display` refusal → fails at "flex".
    #[test]
    fn a_run_is_refused_where_its_handles_would_mislead() {
        let ctx = egui::Context::default();
        let refused = |edit: &dyn Fn(&OndinApp, &[NodeId]) -> Vec<Operation>, what: &str| {
            let (mut app, cards) = app_with_cards(&ctx, [16.0; 3]);
            assert!(app.loose_run().is_some(), "fixture for {what}");
            let ops = edit(&app, &cards);
            assert!(app.session.commit(Transaction(ops)));
            assert!(app.loose_run().is_none(), "{what}");
        };
        refused(
            &|app, cards| {
                let board = app.session.doc.get(cards[0]).unwrap().parent().unwrap();
                vec![Operation::SetDisplay {
                    id: board,
                    display: Some(ondin_core::container::Display::Flex(Default::default())),
                }]
            },
            "flex",
        );
        refused(
            &|_, cards| {
                vec![Operation::SetLocked {
                    id: cards[1],
                    locked: true,
                }]
            },
            "locked",
        );
        refused(
            &|_, cards| {
                vec![Operation::SetTransform {
                    id: cards[2],
                    transform: Affine::translate((228.0, 60.0)) * Affine::rotate(0.3),
                }]
            },
            "turned",
        );

        let (mut app, cards) = app_with_cards(&ctx, [16.0; 3]);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTransform {
                    id: cards[3],
                    transform: Affine::translate((400.0, 200.0)),
                }]))
        );
        app.session.selection.set(vec![cards[0], cards[3]]);
        assert!(app.loose_run().is_none(), "diagonal");
    }
}
