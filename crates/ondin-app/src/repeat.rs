//! *Repeat around origin…* (`docs/design/tools.md`, *Repeat around origin*): a
//! popover beside the Transform card that turns copies of the selection about the transform
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
use ondin_core::{IdSource, NodeId, Transaction, build};

/// The card's width: a popover's.
const W: f32 = crate::app::POPOVER_W;
/// The copies' opacity in the preview, out of 255: the 45% previewed copies wear.
const FADE: u8 = 115;
/// The rows the caption under *Link the copies* always takes: its longest form
/// is two at the card's width.
const CAPTION_ROWS: usize = 2;

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
    /// Whether opening the card brought the pivot's handle up, so leaving puts it
    /// away again.
    raised_pivot: bool,
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
            raised_pivot: false,
            built: None,
        }
    }
}

/// What a preview was built from: the document's revision, the selection, the
/// pivot and the ring.
type Key = (u64, Vec<NodeId>, [u64; 2], usize, u64, bool);

impl RepeatCard {
    /// Put back the count and step a right-click cancelled the scrub of.
    pub(crate) fn restore_scrub(&mut self, count: f64, step_deg: f64) {
        self.count = count;
        self.step_deg = step_deg;
    }

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
        // One card in the popover lane at a time.
        self.close_inspector_popovers();
        self.close_blend();
        self.close_warp();
        if self.repeat.open {
            return;
        }
        self.repeat.open = true;
        self.repeat.built = None;
        // The pivot decides the ring, so its handle comes up with the card, and
        // goes with it if the card raised it.
        let raise = !self.show_pivot && self.session.selection.ids().len() == 1;
        self.repeat.raised_pivot = raise;
        self.show_pivot |= raise;
    }

    /// Whether the gesture preview is the card's ring rather than a gesture.
    pub(crate) fn repeat_preview_standing(&self) -> bool {
        self.repeat.open && self.drag.is_none()
    }

    /// Close the card and take its copies off the canvas.
    pub(crate) fn close_repeat(&mut self) {
        if self.repeat.open {
            self.leave_repeat();
            self.session.clear_gesture_preview();
        }
    }

    /// The card's way out, closed or committed: the pivot's handle goes back
    /// down if the card brought it up.
    fn leave_repeat(&mut self) {
        self.repeat.open = false;
        self.repeat.built = None;
        if std::mem::take(&mut self.repeat.raised_pivot) {
            self.show_pivot = false;
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
        use repeat::NotLinked;
        let doc = &self.session.doc;
        let roots = build::outermost(doc, self.session.selection.ids());
        Some(match repeat::why_not_linked(doc, &roots)? {
            NotLinked::Several => LinkedRefusal {
                why: "Linked copies repeat one layer at a time",
                on: false,
            },
            NotLinked::Linked => LinkedRefusal {
                why: "Copies of a component are linked already",
                on: true,
            },
            NotLinked::Not(why) => LinkedRefusal {
                why: crate::menu::create_refusal(why),
                on: false,
            },
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
            true => repeat::linked(
                &self.session.doc,
                &self.session.resolved,
                &mut self.session.ids,
                roots[0],
                pivot,
                ring,
            )
            .map_err(|e| e.to_string()),
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
        self.leave_repeat();
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
        // A popover opened in the lane after the card takes its place.
        if self.inspector_popover_open() {
            return self.close_repeat();
        }
        self.refresh_repeat_preview();
        let area = self.transform_popover_area(ctx, egui::Id::new("repeat-around"));
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
        // Before the fields: `value_field` applies this frame's drag delta inside
        // the call that reports the drag.
        let before = (card.count, card.step_deg);
        let mut scrubbing = false;
        let mut act = None;
        area.show(ctx, |ui| {
            ui::hold_tab(ui);
            ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                let w = ui::menu_inner_w(W, ui::POPOVER_PAD);
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = ui::POPOVER_SECTION_GAP;
                if ui::popover_head(ui, "Repeat around origin") {
                    act = Some(Act::Close);
                }
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
                            let resp = ui::value_field(
                                ui,
                                egui::vec2(fw, ui::CONTROL_H),
                                Prefix::Icon(icon::COPY_SIMPLE),
                                &mut card.count,
                                Scrub::whole(0.1).range(2.0..=repeat::MAX_COUNT as f64),
                                |d| d.custom_formatter(ui::number(0)),
                            );
                            scrubbing |= resp.dragged();
                            resp.on_hover_text("How many in the ring, the original counted")
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
                            scrubbing |= resp.dragged();
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
                // The caption sits under its switch at the label gap, as an
                // eyebrow sits over its control.
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = ui::SECTION_LABEL_GAP;
                    let link = ui::disable_unless(ui, linked_refusal.is_none(), |ui| {
                        ui::switch_row(ui, "Link the copies", linked, crate::settings::SWITCH_H)
                    })
                    .inner;
                    let link = match &linked_refusal {
                        Some(r) => link.on_disabled_hover_text(r.why),
                        None => link.on_hover_text(
                            "Make the original a component and the copies its instances",
                        ),
                    };
                    if link.clicked() && linked_refusal.is_none() {
                        act = Some(Act::Linked(!linked));
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
                    // Two rows whatever it says, a long name cut with an ellipsis: the
                    // popup keeps its height as the switch and the selection change
                    // the words, since an egui popup never grows back.
                    let font = egui::FontId::proportional(ui::CAPTION_PT);
                    let ink = theme::text::faint();
                    let w = ui.available_width();
                    let mut job = egui::text::LayoutJob::simple(caption, font.clone(), ink, w);
                    job.wrap.max_rows = CAPTION_ROWS;
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    let row_h = ui.fonts_mut(|f| f.row_height(&font));
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(w, row_h * CAPTION_ROWS as f32),
                        egui::Sense::hover(),
                    );
                    ui.painter().galley(rect.min, galley, ink);
                });
                match crate::settings::footer_row(
                    ui,
                    &[
                        ("Cancel", FieldButton::Off),
                        ("Repeat", FieldButton::Primary),
                    ],
                ) {
                    Some(0) => act = Some(Act::Close),
                    Some(_) => act = Some(Act::Commit),
                    None => {}
                }
            });
        });
        // Armed after a right-click's cancel is answered, so the frame it lands on
        // cannot re-arm it.
        if scrubbing && !self.gesture_cancelled && self.in_flight.repeat_scrub.is_none() {
            self.in_flight.repeat_scrub = Some(before);
        }
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
    use ondin_core::{Document, NodeKind, Operation, Pivot};

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

    /// Linked wraps the tick in a group named for it and makes that the main,
    /// since a rectangle cannot be one, and every copy an instance of it, in one
    /// step; the ring wears the tick's name, and the pivot the card raised goes
    /// back down.
    /// flip: `repeat::linked` wrapping with `build::frame` → fails at the group-kind assertion.
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
        assert!(
            matches!(doc.get(main).unwrap().kind(), NodeKind::Group),
            "the main is a group, not a frame"
        );
        assert_eq!(doc.get(main).unwrap().name(), "Tick");
        assert_eq!(doc.get(group).unwrap().name(), "Tick × 6");
        assert!(!app.show_pivot, "the pivot the card raised went down");
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

    /// The card puts down only the pivot handle it raised: one already up when
    /// it opened stays up when it closes.
    /// flip: `leave_repeat` clearing `show_pivot` unconditionally → fails at "stays up".
    #[test]
    fn the_card_leaves_a_pivot_it_did_not_raise() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = tick_app(&ctx);
        app.open_repeat();
        assert!(app.show_pivot, "the card raised it");
        app.close_repeat();
        assert!(!app.show_pivot, "and put it down");
        app.show_pivot = true;
        app.open_repeat();
        app.close_repeat();
        assert!(app.show_pivot, "a pivot already up stays up");
    }

    /// The card keeps its height when *Link the copies* swaps its one-line caption
    /// for the two-line one, so the footer does not move under the pointer.
    /// flip: the caption allocating only the rows its text takes → fails at "one height".
    #[test]
    fn the_card_keeps_its_height_as_the_caption_changes() {
        let ctx = egui::Context::default();
        let (mut app, _, _) = tick_app(&ctx);
        app.open_repeat();
        let height = |app: &mut OndinApp| {
            for _ in 0..4 {
                let _ = pass(&ctx, app, Vec::new());
            }
            ctx.memory(|m| m.area_rect(egui::Id::new("repeat-around")))
                .expect("the card is up")
                .height()
        };
        app.session.repeat_linked = false;
        let plain = height(&mut app);
        app.session.repeat_linked = true;
        assert!(app.repeats_linked(), "the fixture: linking is allowed");
        let linked = height(&mut app);
        assert!(
            (plain - linked).abs() < 0.5,
            "one height: {plain} vs {linked}"
        );
    }

    /// One whole frame of the app, at a window the inspector fits in.
    pub(crate) fn pass(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame))
    }

    /// Drag the field under the eyebrow `label` in the card `card` to the right,
    /// on to `away`, and right-click there. Answers the field's centre.
    pub(crate) fn scrub_then_right_click(
        ctx: &egui::Context,
        app: &mut OndinApp,
        card: egui::Id,
        label: &str,
        away: impl Fn(egui::Pos2) -> egui::Pos2,
        mid: impl Fn(&OndinApp),
    ) -> egui::Pos2 {
        let mut out = pass(ctx, app, Vec::new());
        for _ in 0..10 {
            out = pass(ctx, app, Vec::new());
        }
        let area = ctx.memory(|m| m.area_rect(card)).expect("the card is up");
        let eyebrow = out
            .shapes
            .iter()
            .find_map(|c| match &c.shape {
                egui::Shape::Text(t)
                    if t.galley.text() == label.to_uppercase() && area.contains(t.pos) =>
                {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the {label} eyebrow"));
        let field = egui::pos2(
            eyebrow.left() + 40.0,
            eyebrow.bottom() + ui::SECTION_LABEL_GAP + ui::CONTROL_H / 2.0,
        );
        let button = |pressed, pos, button| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        pass(ctx, app, vec![egui::Event::PointerMoved(field)]);
        pass(
            ctx,
            app,
            vec![button(true, field, egui::PointerButton::Primary)],
        );
        for dx in [10.0, 30.0, 60.0, 90.0] {
            let at = field + egui::vec2(dx, 0.0);
            pass(ctx, app, vec![egui::Event::PointerMoved(at)]);
        }
        let to = away(field + egui::vec2(90.0, 0.0));
        pass(ctx, app, vec![egui::Event::PointerMoved(to)]);
        mid(app);
        pass(
            ctx,
            app,
            vec![button(true, to, egui::PointerButton::Secondary)],
        );
        pass(
            ctx,
            app,
            vec![button(false, to, egui::PointerButton::Secondary)],
        );
        pass(
            ctx,
            app,
            vec![button(false, to, egui::PointerButton::Primary)],
        );
        for _ in 0..3 {
            pass(ctx, app, Vec::new());
        }
        field
    }

    /// The card opens in the popover lane beside the Transform card, level with
    /// its top; a right-click mid-scrub of Count puts the count back, with the
    /// pointer still on the card or out over the canvas.
    /// flip: `cancel_gesture` not restoring `repeat_scrub` → fails at "the count is back".
    #[test]
    fn the_card_hangs_beside_the_transform_card_and_a_right_click_winds_a_scrub_back() {
        let id = egui::Id::new("repeat-around");
        for outside in [false, true] {
            let ctx = egui::Context::default();
            let (mut app, _, _) = tick_app(&ctx);
            pass(&ctx, &mut app, Vec::new());
            app.open_repeat();
            let before = app.repeat.count;
            scrub_then_right_click(
                &ctx,
                &mut app,
                id,
                "Count",
                |at| match outside {
                    true => egui::pos2(200.0, 400.0),
                    false => at,
                },
                |app| {
                    assert_ne!(app.repeat.count, before, "the drag scrubbed the count");
                    assert!(app.in_flight.repeat_scrub.is_some(), "the scrub is armed");
                },
            );
            assert_eq!(app.repeat.count, before, "the count is back ({outside})");
            assert!(app.repeat.open, "the right-click closed nothing");
            let card = ctx.memory(|m| m.area_rect(id)).expect("the card");
            let transform = app.transform_card.expect("the Transform card drawn");
            assert_eq!(card.top(), transform.top(), "level with the Transform card");
            assert_eq!(
                card.right() + crate::app::POPOVER_GAP,
                transform.left(),
                "a popover's gap left of the cards"
            );
        }
    }
}
