//! *Warp…* (`docs/design/tools.md`, *Warp*): an envelope round the selection on
//! the canvas, bent by its corners or by one of three presets, previewed live and
//! baked into plain paths as one *Warp* step (`ondin_core::warp`).
//!
//! The session owns the canvas pointer while it is open: a press on a handle
//! drags it and any other press does nothing, so the selection cannot change
//! under the envelope. Its bar takes the path bar's place (`crate::path_bar`).
//! `Enter` commits and `Escape` cancels, as in point editing.

use crate::app::OndinApp;
use crate::canvas::HANDLE_PICK_PX;
use eframe::egui;
use ondin_core::kurbo::{Point, Rect, Vec2};
use ondin_core::warp::{self, Envelope, Preset, Warp};
use ondin_core::{IdSource, NodeId, NodeKind, Transaction};

/// The status line's word when a drag stops at the fold.
pub(crate) const FOLDS: &str = "That much bend folds the shape";
/// The status line's word when a corner would cross another's edge.
const CORNER_FOLDS: &str = "That corner would fold the shape";
/// Said when *Arc* is picked over text, which a warp outlines.
const TEXT_ON_ARC: &str = "Put the text on a path to keep it editable";

/// An open envelope: what it wraps and how it is bent.
pub(crate) struct WarpSession {
    /// The selection it opened on; another selection closes it.
    selection: Vec<NodeId>,
    /// The document revision it opened on; an edit under it closes it.
    revision: u64,
    bounds: Rect,
    /// Where *Free distort* has put the box's corners, top-left first.
    corners: [Point; 4],
    /// The preset in use, or `None` for *Free distort*.
    pub(crate) preset: Option<Preset>,
    /// The presets' one bend, `-1..=1`, kept as they are switched between.
    pub(crate) bend: f64,
    pub(crate) vertical: bool,
    /// The handle being dragged.
    pub(crate) grab: Option<Grab>,
    /// The corners and bend as the drag in hand found them, which a right-click
    /// puts back.
    grab_from: ([Point; 4], f64),
    /// The preview built for the envelope it shows.
    built: Option<(Envelope, Transaction)>,
}

/// A handle in the hand.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Grab {
    /// A corner, with where it was and the two edge directions Shift keeps it on.
    Corner {
        i: usize,
        from: Point,
        edges: [Vec2; 2],
    },
    /// A preset's bend handle.
    Bend,
}

impl WarpSession {
    /// The envelope as it stands.
    pub(crate) fn warp(&self) -> Warp {
        let envelope = match self.preset {
            None => Envelope::Free(self.corners),
            Some(preset) => Envelope::Preset {
                preset,
                bend: self.bend,
                vertical: self.vertical,
            },
        };
        Warp {
            bounds: self.bounds,
            envelope,
        }
    }

    /// The bend held to what the current preset takes without folding.
    fn held_bend(&self, preset: Preset) -> f64 {
        let max = warp::max_bend(self.bounds, preset, self.vertical);
        self.bend.clamp(-max, max)
    }

    /// The handles' world positions, for the overlay and the press.
    pub(crate) fn handles(&self) -> Vec<Point> {
        match self.warp().handle() {
            Some(h) => vec![h],
            None => self.corners.to_vec(),
        }
    }
}

impl OndinApp {
    /// Why the selection cannot be warped, if it cannot.
    pub(crate) fn warp_refusal(&self) -> Option<&'static str> {
        let ids = self.session.selection.ids();
        if let Err(r) = warp::subjects(&self.session.doc, ids) {
            return Some(r.says());
        }
        warp::bounds(&self.session.resolved, ids)
            .is_none()
            .then(|| warp::Refusal::Nothing.says())
    }

    /// Open the envelope round the selection, or say why it cannot.
    pub(crate) fn open_warp(&mut self) {
        if let Some(why) = self.warp_refusal() {
            return self.session.info(why);
        }
        let selection = self.session.selection.ids().to_vec();
        let Some(bounds) = warp::bounds(&self.session.resolved, &selection) else {
            return;
        };
        // Points editing and the pen would fight it for the canvas and the bar.
        if !self.tool.selects() {
            self.choose_tool(crate::tools::Tool::Select);
        }
        self.close_inspector_popovers();
        self.close_repeat();
        self.close_blend();
        self.warp = Some(WarpSession {
            selection,
            revision: self.session.revision(),
            bounds,
            corners: warp::corners(bounds),
            preset: None,
            bend: 0.4,
            vertical: false,
            grab: None,
            grab_from: (warp::corners(bounds), 0.4),
            built: None,
        });
        self.session.info(
            "Warping: drag a corner, or pick a shape on the bar; Enter to finish, Escape to cancel",
        );
    }

    /// Close the envelope and leave the layers as they were.
    pub(crate) fn close_warp(&mut self) {
        if self.warp.take().is_some() {
            self.session.clear_gesture_preview();
        }
    }

    /// Whether the gesture preview is the envelope's rather than a gesture.
    pub(crate) fn warp_preview_standing(&self) -> bool {
        self.warp.is_some() && self.drag.is_none()
    }

    /// Bake the envelope into the layers, one *Warp* step. An envelope that moves
    /// nothing closes with no step.
    pub(crate) fn commit_warp(&mut self) {
        let Some(s) = self.warp.take() else { return };
        self.session.clear_gesture_preview();
        let w = s.warp();
        if w.is_identity() {
            return;
        }
        let made = warp::warp(
            &self.session.doc,
            &self.session.resolved,
            &mut self.session.ids,
            &s.selection,
            &w,
        );
        let (tx, picked) = match made {
            Ok(made) => made,
            Err(r) => return self.session.info(r.says()),
        };
        self.session.name_step("Warp", crate::theme::icon::WARP);
        if self.commit_edit(tx) {
            self.session.selection.set(picked);
            self.session.info("Warped");
        }
    }

    /// Close the envelope when the selection or the document moved under it, and
    /// keep its preview up. Every frame, before the bar and the canvas read it.
    pub(crate) fn tend_warp(&mut self) {
        let Some(s) = &self.warp else { return };
        if s.selection != self.session.selection.ids() || s.revision != self.session.revision() {
            self.close_warp();
            return;
        }
        self.refresh_warp_preview();
    }

    /// Build the preview again when the envelope changed, and put it back when
    /// something else cleared it. The commit's ids are not spent: the stand-ins
    /// are minted from a copy of the counter.
    fn refresh_warp_preview(&mut self) {
        if !self.drag.is_none() {
            return;
        }
        let Some(s) = &self.warp else { return };
        let w = s.warp();
        let fresh = s.built.as_ref().is_none_or(|(e, _)| *e != w.envelope);
        if fresh {
            let tx = if w.is_identity() {
                Transaction::default()
            } else {
                let mut minter = IdSource::new(self.session.ids.actor());
                minter.skip_to(self.session.ids.next_seq());
                warp::preview(
                    &self.session.doc,
                    &self.session.resolved,
                    &mut minter,
                    &s.selection,
                    &w,
                )
                .unwrap_or_default()
            };
            self.session.set_preview(&tx);
            if let Some(s) = &mut self.warp {
                s.built = Some((w.envelope, tx));
            }
        } else if !self.session.has_gesture_preview()
            && let Some((_, tx)) = &s.built
        {
            let tx = tx.clone();
            self.session.set_preview(&tx);
        }
    }

    /// Pick the envelope's shape from the bar: a preset keeps the bend, held to
    /// what it takes, and *Free distort* keeps its corners.
    pub(crate) fn set_warp_shape(&mut self, preset: Option<Preset>) {
        let Some(s) = &mut self.warp else { return };
        s.preset = preset;
        if let Some(p) = preset {
            s.bend = s.held_bend(p);
        }
        if preset == Some(Preset::Arc) && self.warp_takes_text() {
            self.session.info(TEXT_ON_ARC);
        } else if preset.is_some() {
            // A preset has one handle, not the corners the opening line named.
            self.session
                .info("Warping: drag the handle to bend it; Enter to finish, Escape to cancel");
        }
        self.refresh_warp_preview();
    }

    /// Whether any layer the envelope bends is text, which it outlines.
    fn warp_takes_text(&self) -> bool {
        let Some(s) = &self.warp else { return false };
        warp::subjects(&self.session.doc, &s.selection).is_ok_and(|leaves| {
            leaves.iter().any(|id| {
                matches!(
                    self.session.doc.get(*id).map(|n| n.kind()),
                    Some(NodeKind::Text { .. })
                )
            })
        })
    }

    /// Put the envelope back to square: the corners home and the bend at zero.
    pub(crate) fn reset_warp(&mut self) {
        let Some(s) = &mut self.warp else { return };
        s.corners = warp::corners(s.bounds);
        s.bend = 0.0;
        self.refresh_warp_preview();
    }

    /// Set the presets' bend from the card, held to what the preset takes; asking
    /// for more says so.
    pub(crate) fn set_warp_bend(&mut self, bend: f64) {
        let Some(s) = &mut self.warp else { return };
        let Some(p) = s.preset else { return };
        s.bend = bend.clamp(-1.0, 1.0);
        let held = s.held_bend(p);
        let stopped = (held - s.bend).abs() > 1e-9;
        s.bend = held;
        if stopped {
            self.session.info(FOLDS);
        }
        self.refresh_warp_preview();
    }

    /// Bend along the width or the height, held to what that direction takes.
    pub(crate) fn set_warp_vertical(&mut self, vertical: bool) {
        let Some(s) = &mut self.warp else { return };
        s.vertical = vertical;
        if let Some(p) = s.preset {
            s.bend = s.held_bend(p);
        }
        self.refresh_warp_preview();
    }

    /// The envelope's pointer: a press on a handle takes it, a drag moves it, the
    /// release lets go. Every other press is swallowed.
    pub(crate) fn warp_input(
        &mut self,
        ui: &egui::Ui,
        resp: &egui::Response,
        rect: egui::Rect,
        ppp: f32,
    ) {
        let shift = ui.input(|i| i.modifiers.shift);
        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin());
            let grab = origin.and_then(|p| self.warp_handle_at(p, rect, ppp));
            if let Some(s) = &mut self.warp {
                s.grab = grab;
                s.grab_from = (s.corners, s.bend);
            }
        }
        // A right-click called the drag off; its release must not pick it up again.
        if self.gesture_cancelled {
            return;
        }
        if resp.dragged()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let world = self.to_world(p, rect, ppp);
            self.drag_warp_handle(world, shift);
        }
        if resp.drag_stopped()
            && let Some(s) = &mut self.warp
        {
            s.grab = None;
        }
    }

    /// A right-click mid-drag: the handle in the hand goes back to where its drag
    /// began. Returns whether one was in the hand.
    pub(crate) fn cancel_warp_grab(&mut self) -> bool {
        let Some(s) = &mut self.warp else {
            return false;
        };
        if s.grab.take().is_none() {
            return false;
        }
        (s.corners, s.bend) = s.grab_from;
        self.refresh_warp_preview();
        true
    }

    /// A right-click mid-scrub of the card's Bend: the bend it began from.
    pub(crate) fn restore_warp_bend(&mut self, bend: f64) {
        let Some(s) = &mut self.warp else { return };
        s.bend = bend;
        self.refresh_warp_preview();
    }

    /// The handle under the screen point `p`, if any.
    pub(crate) fn warp_handle_at(&self, p: egui::Pos2, rect: egui::Rect, ppp: f32) -> Option<Grab> {
        let s = self.warp.as_ref()?;
        let near = |w: Point| self.to_screen(w, rect, ppp).distance(p) <= HANDLE_PICK_PX;
        if s.preset.is_some() {
            return s.warp().handle().filter(|h| near(*h)).map(|_| Grab::Bend);
        }
        let i = (0..4).find(|i| near(s.corners[*i]))?;
        let c = s.corners;
        Some(Grab::Corner {
            i,
            from: c[i],
            edges: [c[i] - c[(i + 3) % 4], c[i] - c[(i + 1) % 4]],
        })
    }

    /// Move the handle in the hand to `world`. A corner stops short of folding the
    /// box; Shift keeps it on the line of whichever of its two edges is nearer. The
    /// bend handle sets the bend, held to what the preset takes.
    pub(crate) fn drag_warp_handle(&mut self, world: Point, shift: bool) {
        let Some(s) = &self.warp else { return };
        let Some(grab) = s.grab else { return };
        match grab {
            Grab::Corner { i, from, edges } => {
                let at = if shift {
                    edges
                        .iter()
                        .filter(|e| e.hypot2() > 1e-12)
                        .map(|e| from + *e * ((world - from).dot(*e) / e.hypot2()))
                        .min_by(|a, b| a.distance(world).total_cmp(&b.distance(world)))
                        .unwrap_or(world)
                } else {
                    self.pixel_point(world)
                };
                let mut corners = s.corners;
                corners[i] = at;
                let w = Warp {
                    bounds: s.bounds,
                    envelope: Envelope::Free(corners),
                };
                if w.folds() {
                    self.session.info(CORNER_FOLDS);
                } else if let Some(s) = &mut self.warp {
                    s.corners = corners;
                }
            }
            Grab::Bend => {
                let Some((bend, stopped)) = s.warp().bend_toward(world) else {
                    return;
                };
                if stopped {
                    self.session.info(FOLDS);
                }
                if let Some(s) = &mut self.warp {
                    s.bend = bend;
                }
            }
        }
        self.refresh_warp_preview();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{Document, Operation};

    /// A headless app holding a rectangle and an ellipse, both selected.
    fn app_with_two(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let (rect, ellipse) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(200.0, 60.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((0.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: ellipse,
                parent: page,
                index: 1,
                kind: NodeKind::Ellipse {
                    size: Size::new(40.0, 40.0),
                    arc: Default::default(),
                },
                transform: Some(Affine::translate((80.0, 10.0))),
                name: None,
            },
        ]))
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set(vec![rect, ellipse]);
        (app, rect, ellipse)
    }

    /// A right-click mid-drag of a corner puts the corner back where the drag
    /// began and ends the drag; the envelope stays open.
    /// flip: `cancel_gesture` not calling `cancel_warp_grab` → fails at "a drag
    /// was called off".
    #[test]
    fn a_right_click_puts_a_dragged_corner_back() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = app_with_two(&ctx);
        app.open_warp();
        let s = app.warp.as_mut().expect("opened");
        let home = s.corners;
        s.grab = Some(Grab::Corner {
            i: 1,
            from: home[1],
            edges: [home[1] - home[0], home[1] - home[2]],
        });
        s.grab_from = (home, s.bend);
        app.drag_warp_handle(Point::new(240.0, -30.0), false);
        assert_ne!(
            app.warp.as_ref().unwrap().corners,
            home,
            "the drag moved it"
        );
        assert!(app.cancel_gesture(&ctx), "a drag was called off");
        let s = app.warp.as_ref().expect("still open");
        assert_eq!(s.corners, home, "the corner is back");
        assert!(s.grab.is_none(), "and out of the hand");
    }

    /// A right-click mid-scrub of the card's Bend puts the bend back.
    /// flip: `warp_card` not arming `warp_scrub` → fails at "the scrub is armed".
    #[test]
    fn a_right_click_winds_a_bend_scrub_back() {
        use crate::repeat::tests::{scrub_then_right_click, tick_app};
        let ctx = egui::Context::default();
        let (mut app, _, _) = tick_app(&ctx);
        app.open_warp();
        app.set_warp_shape(Some(Preset::Arc));
        // Under the fold's limit, so the scrub has room to move it.
        app.set_warp_bend(0.05);
        let before = app.warp.as_ref().expect("opened").bend;
        scrub_then_right_click(
            &ctx,
            &mut app,
            egui::Id::new("warp-card"),
            "Bend",
            |at| at,
            |app| {
                let s = app.warp.as_ref().expect("open");
                assert_ne!(s.bend, before, "the drag scrubbed the bend");
                assert!(app.in_flight.warp_scrub.is_some(), "the scrub is armed");
            },
        );
        let s = app.warp.as_ref().expect("the right-click closed nothing");
        assert_eq!(s.bend, before, "the bend is back");
    }

    /// One envelope round the selection; dragging a corner, then committing, bakes
    /// both layers into paths in one *Warp* step that keeps the selection.
    #[test]
    fn a_corner_drag_commits_one_warp_step() {
        let ctx = egui::Context::default();
        let (mut app, rect, ellipse) = app_with_two(&ctx);
        app.open_warp();
        let s = app.warp.as_ref().expect("opened");
        assert_eq!(
            s.bounds,
            Rect::new(0.0, 0.0, 200.0, 60.0),
            "one box round both"
        );
        let depth = app.session.history.undo_depth();

        app.warp.as_mut().unwrap().grab = Some(Grab::Corner {
            i: 1,
            from: Point::new(200.0, 0.0),
            edges: [Vec2::new(200.0, 0.0), Vec2::new(0.0, -60.0)],
        });
        app.drag_warp_handle(Point::new(240.0, -30.0), false);
        assert!(app.session.has_gesture_preview(), "previewed");
        assert!(
            app.session.doc.get(rect).is_some(),
            "the preview deletes nothing"
        );
        app.commit_warp();

        assert!(app.warp.is_none());
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        let done = app.session.history.done().last().expect("a step");
        assert_eq!(done.note.label, "Warp");
        let picked = app.session.selection.ids().to_vec();
        assert_eq!(picked.len(), 2);
        assert!(
            !picked.contains(&rect) && !picked.contains(&ellipse),
            "stand-ins"
        );
        for id in picked {
            let node = app.session.doc.get(id).expect("selected");
            assert!(matches!(node.kind(), NodeKind::Path { .. }));
        }
        let bounds = warp::bounds(&app.session.resolved, app.session.selection.ids()).unwrap();
        assert!((bounds.x1 - 240.0).abs() < 1e-6 && (bounds.y0 + 30.0).abs() < 1e-6);
    }

    /// Escape leaves the layers as they were, with no step and no preview; an
    /// envelope that moves nothing commits nothing.
    #[test]
    fn cancelling_or_an_untouched_envelope_leaves_no_step() {
        let ctx = egui::Context::default();
        let (mut app, rect, _) = app_with_two(&ctx);
        let depth = app.session.history.undo_depth();
        app.open_warp();
        app.set_warp_shape(Some(Preset::Bulge));
        assert!(app.session.has_gesture_preview());
        app.escape(&ctx);
        assert!(app.warp.is_none(), "Escape closes it");
        assert!(
            !app.session.has_gesture_preview(),
            "and takes the preview down"
        );
        assert!(app.session.doc.get(rect).is_some());
        app.open_warp();
        app.commit_warp();
        assert_eq!(app.session.history.undo_depth(), depth, "nothing to bake");
    }

    /// The bend handle stops at the fold and says so; the card's bend is held the
    /// same way, and a preset switch keeps the bend.
    ///
    /// flip: `set_warp_bend` without its `held_bend` → fails at "held".
    #[test]
    fn the_bend_stops_at_the_fold() {
        let ctx = egui::Context::default();
        let (mut app, ..) = app_with_two(&ctx);
        app.session
            .selection
            .set(vec![app.session.selection.ids()[1]]);
        app.open_warp();
        app.set_warp_shape(Some(Preset::Arc));
        let max = warp::max_bend(Rect::new(80.0, 10.0, 120.0, 50.0), Preset::Arc, false);
        assert!(
            max < 1.0,
            "fixture: a square box folds before a half circle"
        );
        app.set_warp_bend(1.0);
        let s = app.warp.as_ref().unwrap();
        assert!((s.bend - max).abs() < 1e-9, "held: {}", s.bend);
        assert_eq!(app.session.status().text, FOLDS);
        app.set_warp_shape(Some(Preset::Flag));
        assert!((app.warp.as_ref().unwrap().bend - max).abs() < 1e-9, "kept");

        app.warp.as_mut().unwrap().grab = Some(Grab::Bend);
        app.set_warp_shape(Some(Preset::Arc));
        app.drag_warp_handle(Point::new(100.0, -5000.0), false);
        assert!((app.warp.as_ref().unwrap().bend - max).abs() < 1e-9);
    }

    /// A selection change or an edit under the envelope closes it.
    #[test]
    fn the_envelope_closes_when_the_selection_changes() {
        let ctx = egui::Context::default();
        let (mut app, rect, _) = app_with_two(&ctx);
        app.open_warp();
        app.tend_warp();
        assert!(app.warp.is_some());
        app.session.selection.set_one(rect);
        app.tend_warp();
        assert!(app.warp.is_none());
    }

    /// Nothing selected refuses with the row's own sentence.
    #[test]
    fn nothing_selected_refuses() {
        let ctx = egui::Context::default();
        let (mut app, ..) = app_with_two(&ctx);
        app.session.selection.clear();
        assert_eq!(app.warp_refusal(), Some("Select layers to warp"));
        app.open_warp();
        assert!(app.warp.is_none());
    }
}
