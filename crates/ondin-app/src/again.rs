//! *Transform again* (`docs/design/tools.md`, *Transform again*): the last
//! move, rotation, scale or skew applied once more to the selection, each layer
//! about its own transform origin.
//!
//! The step is learnt where every door already meets: the session's commit reads
//! an undo step named as a transform ([`Watch`]) and measures what it did to the
//! first layer it touched. So the canvas drags, the arrow keys, the Transform
//! card's fields and buttons, and the Scale row all teach it without each saying so.

use crate::app::OndinApp;
use ondin_core::kurbo::{Affine, Point, Rect, Vec2};
use ondin_core::{NodeId, Resolved, Transaction, build};

/// The transform *Transform again* repeats, measured in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Again {
    Move(Vec2),
    /// Radians, clockwise on screen.
    Rotate(f64),
    /// A factor, `1.2` for 120%.
    Scale(f64),
    /// The world-space lean, translation dropped, and its angle in radians.
    Skew(Affine, f64),
}

/// Under this an angle, a factor's distance from 1 or a move is no transform.
const EPS: f64 = 1e-6;

impl Again {
    /// What the palette row names: *Rotate 15°*, *Move 24, 0*, *Scale 120%*.
    pub(crate) fn label(self) -> String {
        match self {
            Again::Move(v) => format!("Move {}, {}", num(v.x), num(v.y)),
            Again::Rotate(a) => format!("Rotate {}°", num(a.to_degrees())),
            Again::Scale(s) => format!("Scale {}%", num(s * 100.0)),
            Again::Skew(_, a) => format!("Skew {}°", num(a.to_degrees())),
        }
    }

    /// What the status line says once it is applied: *Rotated 15° again*.
    fn said(self) -> String {
        let label = self.label();
        let (verb, rest) = label.split_once(' ').unwrap_or((&label, ""));
        let done = match verb {
            "Move" => "Moved",
            "Rotate" => "Rotated",
            "Scale" => "Scaled",
            _ => "Skewed",
        };
        format!("{done} {rest} again")
    }

    /// The edit applying `self` to `roots` (outermost), each about its own origin.
    fn tx(self, doc: &ondin_core::Document, res: &Resolved, roots: &[NodeId]) -> Transaction {
        if let Again::Move(v) = self {
            return build::offsets(doc, res, roots, &vec![v; roots.len()]);
        }
        let mut ops = Vec::new();
        for &id in roots {
            let Some(p) = ondin_core::pivot_world(doc, res, id) else {
                continue;
            };
            let about =
                |l: Affine| Affine::translate(p.to_vec2()) * l * Affine::translate(-p.to_vec2());
            let one = match self {
                Again::Move(_) => unreachable!("answered above"),
                Again::Rotate(a) => crate::tools::rotate_node(doc, res, id, p, a),
                Again::Skew(l, _) => crate::tools::transform_node(doc, res, id, about(l)),
                Again::Scale(s) => {
                    let Some(b) = res.world_bounds(id) else {
                        continue;
                    };
                    // A box with its top-left at the origin, held there: any box
                    // scales about its held corner, so this one scales about `p`.
                    let size = b.size().to_vec2();
                    crate::tools::resize_selection(
                        doc,
                        res,
                        &[id],
                        crate::tools::SelectionBox::upright(Rect::from_points(p, p + size)),
                        crate::preview::Handle::BottomRight,
                        p + size * s,
                        crate::tools::Resize {
                            keep_ratio: false,
                            symmetric: false,
                            scaling: crate::tools::Scaling::Photographic,
                        },
                    )
                }
            };
            ops.extend(one.0);
        }
        Transaction(ops)
    }
}

/// A number as the fields show one: two places at most, trailing zeros trimmed.
fn num(v: f64) -> String {
    crate::ui::number(2)(v, 0..=2)
}

/// What a step was, read off its name.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Move,
    Rotate,
    Scale,
    Skew,
}

/// The undo-step names that teach *Transform again*, as the doors name them.
fn kind_of(label: &str) -> Option<Kind> {
    match label {
        "Move" | "Nudge" => Some(Kind::Move),
        "Rotate" => Some(Kind::Rotate),
        l if l.starts_with("Rotate 90°") => Some(Kind::Rotate),
        "Scale" => Some(Kind::Scale),
        "Skew" => Some(Kind::Skew),
        _ => None,
    }
}

/// One transform commit being watched: the first layer it names, as it stood.
pub(crate) struct Watch {
    kind: Kind,
    id: NodeId,
    world: Affine,
    bounds: Rect,
}

impl Watch {
    /// Start watching a commit noted `note`, when its name is a transform's.
    pub(crate) fn of(note: &ondin_core::StepNote, res: &Resolved) -> Option<Self> {
        let kind = kind_of(&note.label)?;
        let id = *note.subjects.first()?;
        Some(Self {
            kind,
            id,
            world: res.world_transform(id)?,
            bounds: res.world_bounds(id)?,
        })
    }

    /// What the commit did, read once it has landed; `None` when it did nothing
    /// this can repeat.
    pub(crate) fn finish(self, res: &Resolved) -> Option<Again> {
        let now = res.world_transform(self.id)?;
        let delta = now * self.world.inverse();
        let linear = delta * Affine::translate(-(delta * Point::ZERO).to_vec2());
        let again = match self.kind {
            Kind::Move => Again::Move((now * Point::ZERO) - (self.world * Point::ZERO)),
            Kind::Rotate => {
                let [a, b, ..] = linear.as_coeffs();
                Again::Rotate(b.atan2(a))
            }
            // Named by how far the layer's own lean moved; applied as the world lean.
            Kind::Skew => {
                let lean = |w: Affine| build::Basis::of(w).orientation.skew;
                Again::Skew(linear, lean(now) - lean(self.world))
            }
            // Only a proportional scale repeats; a stretch has no one factor.
            Kind::Scale => {
                let now = res.world_bounds(self.id)?;
                let (was_w, was_h) = (self.bounds.width(), self.bounds.height());
                (was_w > EPS && was_h > EPS).then_some(())?;
                let (sx, sy) = (now.width() / was_w, now.height() / was_h);
                ((sx - sy).abs() < 1e-3).then_some(())?;
                Again::Scale(sx)
            }
        };
        match again {
            Again::Move(v) if v.hypot() < EPS => None,
            Again::Rotate(a) | Again::Skew(_, a) if a.abs() < EPS => None,
            Again::Scale(s) if (s - 1.0).abs() < EPS => None,
            a => Some(a),
        }
    }
}

/// Why *Transform again* is dim with nothing to repeat.
pub(crate) const NOTHING_YET: &str = "Move, rotate or scale something first";

impl OndinApp {
    /// *Transform again*: the remembered step applied once more to every outermost
    /// selected layer, one undo step per application.
    pub(crate) fn transform_again(&mut self) {
        let Some(again) = self.session.last_transform else {
            return self.session.info(NOTHING_YET);
        };
        let roots = build::outermost(&self.session.doc, self.session.selection.ids());
        if roots.is_empty() {
            return self.session.info(crate::menu::NEEDS_SELECTION);
        }
        let tx = again.tx(&self.session.doc, &self.session.resolved, &roots);
        self.name_step_as(crate::menu::Item::TransformAgain);
        if self.commit_edit(tx) {
            // The commit was named *Transform again*, which teaches nothing; the
            // step stays what it was.
            self.session.last_transform = Some(again);
            self.session.info(again.said());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row names the step as a person would say it, and the status turns it
    /// into the past tense.
    #[test]
    fn labels_and_status_read_as_the_spec_words_them() {
        assert_eq!(Again::Rotate(15f64.to_radians()).label(), "Rotate 15°");
        assert_eq!(Again::Move(Vec2::new(24.0, 0.0)).label(), "Move 24, 0");
        assert_eq!(Again::Scale(1.2).label(), "Scale 120%");
        assert_eq!(
            Again::Rotate(15f64.to_radians()).said(),
            "Rotated 15° again"
        );
        assert_eq!(
            Again::Move(Vec2::new(24.0, -8.5)).said(),
            "Moved 24, -8.5 again"
        );
    }

    /// A rotation committed by any door is learnt, and applied to another layer
    /// about that layer's own origin; it survives the selection changing, and
    /// each application is its own step.
    #[test]
    fn a_rotation_is_learnt_at_the_commit_and_applied_again_about_each_origin() {
        let ctx = eframe::egui::Context::default();
        let (mut app, frame, tick) = crate::repeat::tests::tick_app(&ctx);
        let (doc, res) = (&app.session.doc, &app.session.resolved);
        let p = ondin_core::pivot_world(doc, res, tick).unwrap();
        let tx = crate::tools::rotate_node(doc, res, tick, p, 15f64.to_radians());
        app.session.name_step("Rotate", "");
        assert!(app.session.commit(tx));
        let again = app.session.last_transform.expect("the rotation was learnt");
        assert_eq!(again.label(), "Rotate 15°");

        // The frame turns about its own centre, not the tick's origin.
        app.session.selection.set_one(frame);
        let before = app.session.resolved.world_transform(frame).unwrap();
        let centre = app.session.resolved.world_bounds(frame).unwrap().center();
        app.transform_again();
        let after = app.session.resolved.world_transform(frame).unwrap();
        let want = crate::tools::turn_about(centre, 15f64.to_radians()) * before;
        assert!(
            after
                .as_coeffs()
                .iter()
                .zip(want.as_coeffs())
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
        assert_eq!(app.session.status().text, "Rotated 15° again");
        assert_eq!(app.session.last_transform, Some(again), "the step is kept");
        app.undo();
        assert_eq!(app.session.resolved.world_transform(frame), Some(before));
    }

    /// A move is learnt as its world delta, and a scale as its factor, applied
    /// about the layer's origin so a centred one keeps its middle.
    #[test]
    fn a_move_and_a_scale_are_applied_again() {
        let ctx = eframe::egui::Context::default();
        let (mut app, _, tick) = crate::repeat::tests::tick_app(&ctx);
        let was = app.session.resolved.world_bounds(tick).unwrap();
        let (doc, res) = (&app.session.doc, &app.session.resolved);
        let tx = build::offsets(doc, res, &[tick], &[Vec2::new(24.0, 0.0)]);
        app.session.name_step("Move", "");
        app.session.commit(tx);
        assert_eq!(
            app.session.last_transform,
            Some(Again::Move(Vec2::new(24.0, 0.0)))
        );
        app.transform_again();
        let now = app.session.resolved.world_bounds(tick).unwrap();
        assert!((now.x0 - was.x0 - 48.0).abs() < 1e-6, "moved twice");

        // The origin back at the centre, then a 120% scale learnt and repeated.
        app.session
            .commit(Transaction(vec![ondin_core::Operation::SetPivot {
                id: tick,
                pivot: None,
            }]));
        app.session.last_transform = Some(Again::Scale(1.2));
        let mid = now.center();
        app.transform_again();
        let scaled = app.session.resolved.world_bounds(tick).unwrap();
        assert!((scaled.width() - now.width() * 1.2).abs() < 1e-6);
        assert!(
            (scaled.center() - mid).hypot() < 1e-6,
            "scaled about its centre"
        );
    }

    /// With nothing learnt the verb only says what it needs.
    #[test]
    fn nothing_to_repeat_says_so() {
        let ctx = eframe::egui::Context::default();
        let (mut app, _, _) = crate::repeat::tests::tick_app(&ctx);
        let depth = app.session.history.undo_depth();
        app.transform_again();
        assert_eq!(app.session.status().text, NOTHING_YET);
        assert_eq!(app.session.history.undo_depth(), depth);
    }

    /// The names the doors give their transform steps are the ones watched.
    #[test]
    fn transform_steps_are_recognised_by_name() {
        assert_eq!(kind_of("Move"), Some(Kind::Move));
        assert_eq!(kind_of("Nudge"), Some(Kind::Move));
        assert_eq!(kind_of("Rotate 90° clockwise"), Some(Kind::Rotate));
        assert_eq!(kind_of("Scale"), Some(Kind::Scale));
        assert_eq!(kind_of("Resize"), None);
        assert_eq!(kind_of("Transform again"), None);
    }
}
