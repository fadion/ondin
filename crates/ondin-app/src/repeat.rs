//! *Repeat around origin…* (`docs/design/tools.md`, *Repeat around origin*): a
//! card over the canvas that turns copies of the selection about the transform
//! origin, previewed live, and commits them grouped with the original as one
//! *Repeat* step (`ondin_core::repeat`).
//!
//! Not a modal: the pivot stays draggable while the card is up, because the pivot
//! decides the result, and the preview is built again when it moves.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton, Prefix, Scrub};
use eframe::egui;
use ondin_core::kurbo::Point;
use ondin_core::repeat::{self, Place, Ring};
use ondin_core::{IdSource, NodeId, Operation, Transaction, build};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;
/// The copies' opacity in the preview, out of 255: the 45% previewed copies wear.
const FADE: u8 = 115;

/// The card's settings, kept closed rather than dropped so it reopens on the ring
/// last made.
pub(crate) struct RepeatCard {
    pub(crate) open: bool,
    /// *Fixed angle* rather than *Full turn*.
    fixed: bool,
    count: f64,
    /// The step under *Fixed angle*, in degrees.
    step_deg: f64,
    turn: bool,
    /// The preview built for the key it was built from.
    built: Option<(Key, Transaction)>,
}

impl Default for RepeatCard {
    fn default() -> Self {
        Self {
            open: false,
            fixed: false,
            count: 12.0,
            step_deg: 30.0,
            turn: true,
            built: None,
        }
    }
}

/// What a preview was built from: the document's revision, the selection, the
/// pivot and the ring.
type Key = (u64, Vec<NodeId>, [u64; 2], usize, u64, bool);

impl RepeatCard {
    fn count(&self) -> usize {
        (self.count.round() as usize).clamp(2, repeat::MAX_COUNT)
    }

    fn ring(&self) -> Ring {
        match self.fixed {
            true => Ring {
                count: self.count(),
                step: self.step_deg.to_radians(),
                turn: self.turn,
            },
            false => Ring::full(self.count(), self.turn),
        }
    }
}

/// What a frame of the card asked for.
enum Act {
    Close,
    Commit,
    Fixed(bool),
    Linked(bool),
    Turn,
}

/// Why *Linked* cannot apply, and whether it is forced on rather than off.
struct LinkedRefusal {
    why: &'static str,
    on: bool,
}

impl OndinApp {
    /// Open the card on the selection, or say why it cannot repeat.
    pub(crate) fn open_repeat(&mut self) {
        if let Some(why) = self.repeat_refusal() {
            return self.session.info(why);
        }
        self.close_find();
        self.colour_review.open = false;
        self.close_blend();
        self.repeat.open = true;
        self.repeat.built = None;
        // The pivot decides the ring, so its handle comes up with the card.
        self.show_pivot = self.session.selection.ids().len() == 1;
    }

    /// Whether the gesture preview is the card's ring rather than a gesture.
    pub(crate) fn repeat_preview_standing(&self) -> bool {
        self.repeat.open && self.drag.is_none()
    }

    /// Close the card and take its copies off the canvas.
    pub(crate) fn close_repeat(&mut self) {
        if self.repeat.open {
            self.repeat.open = false;
            self.repeat.built = None;
            self.session.clear_gesture_preview();
        }
    }

    /// Why the selection cannot be repeated: nothing selected, a locked layer, or
    /// layers under more than one parent, which no one group could take.
    pub(crate) fn repeat_refusal(&self) -> Option<&'static str> {
        let doc = &self.session.doc;
        let roots = build::outermost(doc, self.session.selection.ids());
        let Some(first) = roots.first() else {
            return Some(crate::menu::NEEDS_SELECTION);
        };
        if roots
            .iter()
            .any(|id| doc.get(*id).is_some_and(|n| n.locked()))
        {
            return Some("This layer is locked");
        }
        if roots
            .iter()
            .any(|id| ondin_core::is_effectively_locked(doc, *id))
        {
            return Some("A group containing this layer is locked");
        }
        let parent = doc.get(*first).and_then(|n| n.parent());
        if roots
            .iter()
            .any(|id| doc.get(*id).and_then(|n| n.parent()) != parent)
        {
            return Some("Repeat takes layers that share one parent");
        }
        None
    }

    /// The point the ring turns about: one layer's transform origin, or the middle
    /// of a selection's box, which has no shared origin.
    fn repeat_pivot(&self, roots: &[NodeId]) -> Option<Point> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        match roots {
            [one] => ondin_core::pivot_world(doc, res, *one),
            _ => repeat::centre(res, roots),
        }
    }

    /// Why *Linked* cannot apply to the selection, if it cannot.
    fn linked_refusal(&self) -> Option<LinkedRefusal> {
        let doc = &self.session.doc;
        let roots = build::outermost(doc, self.session.selection.ids());
        let [one] = roots.as_slice() else {
            return Some(LinkedRefusal {
                why: "Linked copies repeat one layer at a time",
                on: false,
            });
        };
        let node = doc.get(*one)?;
        if node.component() || ondin_core::component::instance_root(doc, *one) == Some(*one) {
            return Some(LinkedRefusal {
                why: "Copies of a component are linked already",
                on: true,
            });
        }
        self.create_component_refusal().map(|why| LinkedRefusal {
            why: crate::menu::create_refusal(why),
            on: false,
        })
    }

    /// Whether the commit makes the original a component first.
    fn repeats_linked(&self) -> bool {
        self.session.repeat_linked && self.linked_refusal().is_none()
    }

    /// Build the preview again when what it shows has changed, and keep it on the
    /// canvas between other gestures.
    fn refresh_repeat_preview(&mut self) {
        let roots = build::outermost(&self.session.doc, self.session.selection.ids());
        let Some(pivot) = self.repeat_pivot(&roots) else {
            return;
        };
        let ring = self.repeat.ring();
        let key: Key = (
            self.session.revision(),
            roots.clone(),
            [pivot.x.to_bits(), pivot.y.to_bits()],
            ring.count,
            ring.step.to_bits(),
            ring.turn,
        );
        // A canvas drag owns the preview while it lasts.
        if !self.drag.is_none() {
            return;
        }
        let stale = self.repeat.built.as_ref().is_none_or(|(k, _)| *k != key);
        if stale {
            // Minted from a copy of the counter: a preview spends no ids.
            let mut ids = IdSource::new(self.session.ids.actor());
            ids.skip_to(self.session.ids.next_seq());
            let (doc, res) = (&self.session.doc, &self.session.resolved);
            let tx = repeat::repeat(
                doc,
                res,
                &mut ids,
                &roots,
                pivot,
                ring,
                Place::Beside { fade: FADE },
            )
            .map(|(tx, _)| tx)
            .unwrap_or_default();
            self.session.set_preview(&tx);
            self.repeat.built = Some((key, tx));
        } else if !self.session.has_gesture_preview()
            && let Some((_, tx)) = &self.repeat.built
        {
            let tx = tx.clone();
            self.session.set_preview(&tx);
        }
    }

    /// Repeat the selection as the card says: one *Repeat* step that makes the
    /// group, and for *Linked* the component first.
    pub(crate) fn commit_repeat(&mut self) {
        if let Some(why) = self.repeat_refusal() {
            return self.session.info(why);
        }
        let roots = build::outermost(&self.session.doc, self.session.selection.ids());
        let Some(pivot) = self.repeat_pivot(&roots) else {
            return;
        };
        let ring = self.repeat.ring();
        let linked = self.repeats_linked();
        let made = match linked {
            true => self.linked_ring(roots[0], pivot, ring),
            false => repeat::repeat(
                &self.session.doc,
                &self.session.resolved,
                &mut self.session.ids,
                &roots,
                pivot,
                ring,
                Place::Group,
            )
            .map_err(|e| e.to_string()),
        };
        let (tx, group) = match made {
            Ok((tx, Some(group))) => (tx, group),
            Ok((_, None)) => return,
            Err(e) => return self.session.fail(format!("Cannot repeat: {e}")),
        };
        self.session.clear_gesture_preview();
        self.session.name_step("Repeat", icon::REPEAT_AROUND);
        if !self.commit_edit(tx) {
            return;
        }
        self.session.selection.set_one(group);
        let said = match linked {
            true => {
                let name = crate::private::layer(&self.session.doc, roots[0]).into_owned();
                let copies = ui::count_as(ring.count - 1, "linked copy", "linked copies");
                format!("Made {name} a component and {copies} around the origin")
            }
            false => format!(
                "Made {} around the origin",
                ui::count_as(ring.count - 1, "copy", "copies")
            ),
        };
        self.session.info(said);
        self.repeat.open = false;
        self.repeat.built = None;
    }

    /// The edit that makes `id` a component where it stands, or wraps it in a
    /// frame made the component, then repeats that main: its copies land as
    /// instances. Worked on a scratch copy so both halves are one step.
    fn linked_ring(
        &mut self,
        id: NodeId,
        pivot: Point,
        ring: Ring,
    ) -> Result<(Transaction, Option<NodeId>), String> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let (mut tx, main) = match crate::app::create_in_place(doc, id) {
            true => (Transaction(Vec::new()), id),
            false => {
                build::frame(doc, res, &mut self.session.ids, &[id]).map_err(|e| e.to_string())?
            }
        };
        tx.0.push(Operation::SetComponent {
            id: main,
            component: true,
        });
        let mut after = doc.clone();
        let outcome = after.apply(&tx).map_err(|e| e.to_string())?;
        let mut laid = res.clone();
        laid.update(&after, &outcome.dirty);
        let (more, group) = repeat::repeat(
            &after,
            &laid,
            &mut self.session.ids,
            &[main],
            pivot,
            ring,
            Place::Group,
        )
        .map_err(|e| e.to_string())?;
        tx.0.extend(more.0);
        Ok((tx, group))
    }

    /// Draw the card when it is open, keep its preview up, and act on this
    /// frame's clicks.
    pub(crate) fn repeat_ui(&mut self, ctx: &egui::Context) {
        if !self.repeat.open {
            return;
        }
        // The selection went, or became one the card cannot take.
        if self.repeat_refusal().is_some() {
            return self.close_repeat();
        }
        self.refresh_repeat_preview();
        let canvas = self.canvas_rect;
        let linked_refusal = self.linked_refusal();
        let linked = match &linked_refusal {
            Some(r) => r.on,
            None => self.session.repeat_linked,
        };
        let roots = build::outermost(&self.session.doc, self.session.selection.ids());
        let name = roots
            .first()
            .map(|id| crate::private::layer(&self.session.doc, *id).into_owned())
            .unwrap_or_default();
        let card = &mut self.repeat;
        let mut act = None;
        ui::canvas_popup_area(ctx, egui::Id::new("repeat-around"), canvas).show(ctx, |ui| {
            ui::hold_tab(ui);
            crate::settings::card_frame().show(ui, |ui| {
                ui.set_width(W - 2.0 * ui::POPOVER_PAD);
                ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                if crate::settings::modal_title(ui, "Repeat around origin") {
                    act = Some(Act::Close);
                }
                let w = ui.available_width();
                let lit = usize::from(card.fixed);
                if let Some(i) = ui::segmented(ui, w, ui::SEGMENT_CELL_H, 2, lit, |p, i, r, on| {
                    ui::segment_label(p, r, ["Full turn", "Fixed angle"][i], on);
                }) {
                    act = Some(Act::Fixed(i == 1));
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    let fw = (w - ui::CARD_COL_GAP) / 2.0;
                    ui.vertical(|ui| {
                        ui.set_width(fw);
                        ui::labelled(ui, "Count", |ui| {
                            ui::value_field(
                                ui,
                                egui::vec2(fw, ui::CONTROL_H),
                                Prefix::Icon(icon::COPY_SIMPLE),
                                &mut card.count,
                                Scrub::whole(0.1).range(2.0..=repeat::MAX_COUNT as f64),
                                |d| d.custom_formatter(ui::number(0)),
                            )
                            .on_hover_text("How many in the ring, the original counted")
                        });
                    });
                    ui.vertical(|ui| {
                        ui.set_width(fw);
                        ui::labelled(ui, "Step", |ui| {
                            // Under *Full turn* the step is what the count works out.
                            let mut shown = match card.fixed {
                                true => card.step_deg,
                                false => 360.0 / card.count().max(1) as f64,
                            };
                            let resp = ui::disable_unless(ui, card.fixed, |ui| {
                                ui::value_field(
                                    ui,
                                    egui::vec2(fw, ui::CONTROL_H),
                                    Prefix::Icon(icon::ROTATION),
                                    &mut shown,
                                    Scrub::whole(0.5).range(-360.0..=360.0),
                                    |d| d.suffix("°").custom_formatter(ui::number(2)),
                                )
                            })
                            .inner;
                            if card.fixed {
                                card.step_deg = shown;
                            }
                            resp.on_hover_text(match card.fixed {
                                true => "The turn from one copy to the next",
                                false => "A full turn split by the count",
                            })
                        });
                    });
                });
                if ui::switch_row(
                    ui,
                    "Turn the copies with the step",
                    card.turn,
                    crate::settings::SWITCH_H,
                )
                .clicked()
                {
                    act = Some(Act::Turn);
                }
                ui::labelled(ui, "Copies", |ui| {
                    let enabled = linked_refusal.is_none();
                    let tip: &'static str = linked_refusal.as_ref().map_or("", |r| r.why);
                    if let Some(i) = ui::segmented_tipped(
                        ui,
                        w,
                        ui::SEGMENT_CELL_H,
                        2,
                        usize::from(linked),
                        |_| enabled,
                        |_| tip,
                        |p, i, r, on| match enabled {
                            true => ui::segment_label(p, r, ["Plain", "Linked"][i], on),
                            false => ui::segment_label_disabled(p, r, ["Plain", "Linked"][i]),
                        },
                    ) {
                        act = Some(Act::Linked(i == 1));
                    }
                    let caption = match (&linked_refusal, linked) {
                        (Some(r), _) => r.why.to_string(),
                        (None, true) => match card.count() - 1 {
                            1 => format!("{name} becomes a component. The copy is its instance."),
                            n => format!(
                                "{name} becomes a component. The {n} copies are its instances."
                            ),
                        },
                        (None, false) => "Each copy is its own layer.".to_string(),
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(caption)
                                .size(ui::CAPTION_PT)
                                .color(theme::text::faint()),
                        )
                        .wrap(),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                    if crate::settings::text_button(ui, "Repeat", FieldButton::Primary).clicked() {
                        act = Some(Act::Commit);
                    }
                    if crate::settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                        act = Some(Act::Close);
                    }
                });
            });
        });
        let Some(act) = act else {
            return;
        };
        match act {
            Act::Close => self.close_repeat(),
            Act::Commit => self.commit_repeat(),
            Act::Fixed(fixed) => {
                // The step a fixed angle starts from is the full turn's.
                if fixed && !self.repeat.fixed {
                    self.repeat.step_deg = 360.0 / self.repeat.count() as f64;
                }
                self.repeat.fixed = fixed;
            }
            Act::Linked(on) => self.session.repeat_linked = on,
            Act::Turn => self.repeat.turn = !self.repeat.turn,
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size, Vec2};
    use ondin_core::{Document, NodeKind, Pivot};

    /// A headless app holding a frame with an 18 × 58 tick at (311, 72), its
    /// transform origin moved 70 below its box to (320, 200), and the tick
    /// selected. Answers the app, the frame and the tick.
    pub(crate) fn tick_app(ctx: &egui::Context) -> (OndinApp, NodeId, NodeId) {
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, tick) = (ids.mint(), ids.mint());
        let tx = Transaction(vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(640.0, 400.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: tick,
                parent: frame,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(18.0, 58.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((311.0, 72.0))),
                name: Some("Tick".into()),
            },
            Operation::SetPivot {
                id: tick,
                pivot: Some(Pivot::Local(Point::new(9.0, 128.0))),
            },
        ]);
        doc.apply(&tx).expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.canvas_px = (800, 600);
        app.session.selection.set_one(tick);
        (app, frame, tick)
    }

    fn world(app: &OndinApp, id: NodeId) -> Affine {
        app.session.resolved.world_transform(id).expect("placed")
    }

    fn near(a: Affine, b: Affine) -> bool {
        a.as_coeffs()
            .iter()
            .zip(b.as_coeffs())
            .all(|(x, y)| (x - y).abs() < 1e-6)
    }

    /// Twelve round the moved origin: one group named for the tick and the count,
    /// the tick on top, the next copy turned 30° about (320, 200), all one step.
    #[test]
    fn a_full_turn_of_twelve_groups_the_original_over_eleven_turned_copies() {
        let ctx = egui::Context::default();
        let (mut app, frame, tick) = tick_app(&ctx);
        let was = world(&app, tick);
        app.open_repeat();
        assert!(app.repeat.open, "the card opened");
        app.commit_repeat();

        let group = app
            .session
            .selection
            .single()
            .expect("the group is selected");
        let doc = &app.session.doc;
        assert_eq!(doc.get(group).unwrap().name(), "Tick × 12");
        assert_eq!(doc.get(group).unwrap().parent(), Some(frame));
        let kids = doc.get(group).unwrap().children().to_vec();
        assert_eq!(kids.len(), 12);
        assert_eq!(
            kids.last(),
            Some(&tick),
            "the original stays first in the layers"
        );
        let next = kids[kids.len() - 2];
        let turn = crate::tools::turn_about(Point::new(320.0, 200.0), 30f64.to_radians());
        assert!(
            near(world(&app, next), turn * was),
            "the next copy turned 30°"
        );
        assert!(near(world(&app, tick), was), "the original did not move");
        assert_eq!(
            app.session.status().text,
            "Made 11 copies around the origin"
        );
        assert!(!app.repeat.open, "the card closed");

        app.undo();
        let doc = &app.session.doc;
        assert_eq!(
            doc.get(frame).unwrap().children(),
            &[tick],
            "one step undoes it"
        );
    }

    /// Off, the copies travel round without turning: each is the tick moved.
    #[test]
    fn copies_not_turned_keep_the_originals_orientation() {
        let ctx = egui::Context::default();
        let (mut app, _, tick) = tick_app(&ctx);
        let was = world(&app, tick);
        app.open_repeat();
        app.repeat.turn = false;
        app.repeat.count = 4.0;
        app.commit_repeat();
        let group = app.session.selection.single().unwrap();
        let kids = app.session.doc.get(group).unwrap().children().to_vec();
        assert_eq!(kids.len(), 4);
        for k in &kids {
            let w = world(&app, *k);
            assert!(near(
                w * Affine::translate(-w.translation()),
                Affine::IDENTITY
            ));
        }
        // A quarter turn about (320, 200) carries the tick's middle (320, 101)
        // to (419, 200).
        let quarter = world(&app, kids[kids.len() - 2]);
        assert!(near(
            quarter,
            Affine::translate(Vec2::new(99.0, 99.0)) * was
        ));
    }

    /// Linked makes the tick a component where it stands, wrapped in a frame
    /// since a rectangle cannot be one, and every copy an instance of it — in one
    /// step.
    #[test]
    fn linked_wraps_the_tick_in_a_main_and_lands_the_copies_as_its_instances() {
        let ctx = egui::Context::default();
        let (mut app, frame, tick) = tick_app(&ctx);
        app.open_repeat();
        app.session.repeat_linked = true;
        app.repeat.count = 6.0;
        app.commit_repeat();

        let doc = &app.session.doc;
        let group = app.session.selection.single().unwrap();
        let kids = doc.get(group).unwrap().children().to_vec();
        let main = *kids.last().unwrap();
        assert!(
            doc.get(main).unwrap().component(),
            "the original became the main"
        );
        assert_eq!(doc.get(main).unwrap().children(), &[tick]);
        for copy in &kids[..kids.len() - 1] {
            assert_eq!(
                ondin_core::component::main_of(doc, *copy),
                Some(main),
                "every copy is an instance of the main"
            );
        }
        assert!(
            app.session
                .status()
                .text
                .ends_with("a component and 5 linked copies around the origin"),
            "{}",
            app.session.status().text
        );
        app.undo();
        assert_eq!(app.session.doc.get(frame).unwrap().children(), &[tick]);
        assert!(!app.session.doc.get(tick).unwrap().component());
    }

    /// The preview hangs ghosts beside the tick without spending an id, and
    /// Escape takes them away with the card.
    #[test]
    fn the_preview_spends_no_ids_and_leaves_with_the_card() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = tick_app(&ctx);
        let next = app.session.ids.next_seq();
        app.open_repeat();
        app.refresh_repeat_preview();
        assert!(app.session.has_gesture_preview(), "the copies preview");
        assert_eq!(app.session.ids.next_seq(), next);
        app.escape(&ctx);
        assert!(!app.repeat.open);
        assert!(!app.session.has_gesture_preview());
    }

    /// The ring standing on the canvas is no gesture: a right-click's cancel
    /// leaves it, so the click still opens its menu.
    ///
    /// flip: `repeat_preview_standing` answering `false` → fails at the first assertion.
    #[test]
    fn a_right_click_does_not_take_the_ring_for_a_gesture() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = tick_app(&ctx);
        app.open_repeat();
        app.refresh_repeat_preview();
        assert!(!app.cancel_gesture(&ctx), "nothing was in flight");
        assert!(!app.gesture_cancelled);
        assert!(app.session.has_gesture_preview(), "the ring stays up");
    }

    /// A main has Linked on and dimmed, since its copies are instances anyway.
    #[test]
    fn linked_is_refused_with_the_reason_for_the_selection() {
        let ctx = egui::Context::default();
        let (mut app, frame, _) = tick_app(&ctx);
        assert!(
            app.linked_refusal().is_none(),
            "one plain rectangle can be wrapped"
        );
        app.session.selection.set_one(frame);
        app.create_component();
        let r = app.linked_refusal().expect("a main is linked already");
        assert!(r.on);
        assert_eq!(r.why, "Copies of a component are linked already");
    }
}
