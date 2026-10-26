//! Undo/redo (§5.8).
//!
//! Because `apply` always returns the inverse, undo/redo are symmetric with no
//! special cases. History lives in core and is shared by app and MCP: one
//! timeline, into which user and AI edits interleave.
//!
//! One transaction is one undo step, except a **run** ([`History::commit_into_run`])
//! folds a burst of edits (arrow-key nudges) into one. The policy — which edits
//! belong to one burst — is the caller's.
//!
//! Each step carries a [`StepNote`] its caller wrote at the commit, so the
//! history can be listed as well as walked.

use crate::document::Document;
use crate::id::NodeId;
use crate::op::{DirtySet, OpError, Operation, Transaction};
use std::mem::Discriminant;

/// The shape of a commit: which operation, on which node, in which order.
///
/// Two commits may merge only if their shapes are **equal**: the second edit then
/// overwrote exactly the fields the first did, so the first inverse still
/// restores both. A shape is `None` for a transaction containing an operation
/// that is not a plain overwrite (`Operation::shape_key`); a create or reparent
/// is never folded away.
///
/// The field inside the operation is part of the key (§15 D482): a discriminant
/// names the variant, not what it writes, which is false for `SetGeometry`,
/// whose payload is itself a field selector.
type Shape = Vec<(
    Discriminant<Operation>,
    Option<Discriminant<crate::op::GeometryPatch>>,
    NodeId,
)>;

fn shape(tx: &Transaction) -> Option<Shape> {
    if tx.0.is_empty() {
        return None;
    }
    tx.0.iter().map(|op| op.shape_key()).collect()
}

/// Where an undo step's edit came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    #[default]
    User,
    /// An edit an assistant made over MCP: the client that sent it and the tool
    /// it called.
    Mcp { client: String, tool: String },
}

/// What an undo step says about itself, recorded when it was committed: the
/// name its edit gave (never derived from the operations), where it came from,
/// and what it touched. History only keeps it; a list of the history reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepNote {
    /// The command's name as the menus say it, or the MCP tool's.
    pub label: String,
    /// The glyph a list draws beside the label, as its icon font spells it; empty
    /// for none.
    pub glyph: &'static str,
    pub origin: Origin,
    /// The nodes the edit named, in the order it named them.
    pub subjects: Vec<NodeId>,
    /// The first subject's name as it was at the commit.
    pub subject: Option<String>,
    /// The page the subjects were on at the commit.
    pub page: Option<NodeId>,
}

/// One undo step: the transaction that takes it back (or, on the redo stack,
/// brings it back) and its note. `serial` names the step for as long as it
/// lives, across undo and redo.
struct Step {
    tx: Transaction,
    note: StepNote,
    serial: u64,
}

/// A step as the history lists it.
#[derive(Clone, Copy, Debug)]
pub struct StepView<'a> {
    pub note: &'a StepNote,
    pub serial: u64,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The shape of the most recent commit while it is open to merging. Cleared
    /// by an undo, a redo, a commit outside a run, or `end_run`.
    run: Option<Shape>,
    /// The serial the next step takes; never reused, so a serial dropped with the
    /// redo stack cannot come back naming another step.
    next_serial: u64,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply `tx`, push its inverse to the undo stack, and clear the redo stack.
    /// On error the document is untouched (atomic `apply`) and history unchanged.
    /// Returns the `DirtySet` so the caller can update its `Resolved` (§9.2).
    ///
    /// Ends any run in progress; the commit itself becomes the run a following
    /// [`Self::commit_into_run`] may merge into, so the caller need not know
    /// whether it is on the first edit of a burst.
    pub fn commit(&mut self, doc: &mut Document, tx: Transaction) -> Result<DirtySet, OpError> {
        self.commit_noted(doc, tx, StepNote::default())
    }

    /// [`Self::commit`], with the note the new step carries.
    pub fn commit_noted(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
        note: StepNote,
    ) -> Result<DirtySet, OpError> {
        let shape = shape(&tx);
        let outcome = doc.apply(&tx)?;
        self.next_serial += 1;
        self.undo.push(Step {
            tx: outcome.inverse,
            note,
            serial: self.next_serial,
        });
        self.redo.clear();
        self.run = shape;
        Ok(outcome.dirty)
    }

    /// Apply `tx` and fold it into the previous undo step instead of pushing one
    /// of its own — when that is sound, and plain [`Self::commit`] when it is not.
    ///
    /// The merge keeps the older inverse and drops the new one: `A` then `B`
    /// overwriting the same fields leaves `B`'s state, and `A`'s inverse already
    /// restores what was there before either. That needs the *same* fields of the
    /// *same* nodes, which [`Shape`] checks; anything else becomes a fresh step.
    /// The caller decides when a run continues.
    pub fn commit_into_run(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
    ) -> Result<DirtySet, OpError> {
        self.commit_into_run_noted(doc, tx, StepNote::default())
    }

    /// [`Self::commit_into_run`]; `note` is used only when the edit starts a step
    /// of its own, since a merged edit keeps the run's first note.
    pub fn commit_into_run_noted(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
        note: StepNote,
    ) -> Result<DirtySet, OpError> {
        let shape = shape(&tx);
        if shape.is_none() || shape != self.run || !self.can_undo() {
            return self.commit_noted(doc, tx, note);
        }
        // This edit's inverse is discarded: it would restore where the previous
        // edit left off, not where the run began.
        let outcome = doc.apply(&tx)?;
        self.redo.clear();
        Ok(outcome.dirty)
    }

    /// Apply `tx` and fold it into the step on top of the history, keeping that
    /// step's inverse — for an edit the step's undo already takes away whole (a
    /// page added, then named: removing the page removes its name). The caller
    /// vouches for that; [`Self::commit_into_run`] checks shapes instead. A plain
    /// [`Self::commit`] when there is no step to fold into.
    pub fn fold_into_last(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
    ) -> Result<DirtySet, OpError> {
        self.fold_into_last_noted(doc, tx, StepNote::default())
    }

    /// [`Self::fold_into_last`]; `note` is used only when there is no step to
    /// fold into, since the folded edit belongs to the step's own note.
    pub fn fold_into_last_noted(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
        note: StepNote,
    ) -> Result<DirtySet, OpError> {
        if !self.can_undo() {
            return self.commit_noted(doc, tx, note);
        }
        let outcome = doc.apply(&tx)?;
        self.redo.clear();
        self.run = None;
        Ok(outcome.dirty)
    }

    /// End the current run, so the next [`Self::commit_into_run`] starts a step
    /// of its own. The caller's half of the policy.
    pub fn end_run(&mut self) {
        self.run = None;
    }

    /// Undo the most recent transaction, returning its `DirtySet` (or `None` if
    /// there was nothing to undo). The inverse is pushed to the redo stack. A step
    /// that fails to apply stays where it was: `apply` changed nothing, and
    /// dropping it would lose what it held.
    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<DirtySet>, OpError> {
        self.run = None;
        let Some(step) = self.undo.pop() else {
            return Ok(None);
        };
        // A step puts back values the document held, including a layout value a
        // file carried that CSS refuses (§15 D937).
        let outcome = match doc.apply_restoring(&step.tx) {
            Ok(outcome) => outcome,
            Err(e) => {
                self.undo.push(step);
                return Err(e);
            }
        };
        self.redo.push(Step {
            tx: outcome.inverse,
            ..step
        });
        Ok(Some(outcome.dirty))
    }

    /// Redo the most recently undone transaction, returning its `DirtySet` (or
    /// `None` if there was nothing to redo). A failed step stays, as in
    /// [`Self::undo`].
    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<DirtySet>, OpError> {
        // Before the `pop`, and that placement is the point: a redo that restores a
        // step always finds `run` already `None`, so the only reachable effect is
        // that a redo with nothing to redo ends the run in progress.
        self.run = None;
        let Some(step) = self.redo.pop() else {
            return Ok(None);
        };
        let outcome = match doc.apply_restoring(&step.tx) {
            Ok(outcome) => outcome,
            Err(e) => {
                self.redo.push(step);
                return Err(e);
            }
        };
        self.undo.push(Step {
            tx: outcome.inverse,
            ..step
        });
        Ok(Some(outcome.dirty))
    }

    /// The transaction [`Self::undo`] would apply next, for a caller that must
    /// know what it touches before it runs (the app's page switch).
    pub fn next_undo(&self) -> Option<&Transaction> {
        self.undo.last().map(|s| &s.tx)
    }

    /// The transaction [`Self::redo`] would apply next.
    pub fn next_redo(&self) -> Option<&Transaction> {
        self.redo.last().map(|s| &s.tx)
    }

    /// The steps that are done, oldest first: what [`Self::undo`] would take back,
    /// in the reverse of the order it would.
    pub fn done(&self) -> impl DoubleEndedIterator<Item = StepView<'_>> + ExactSizeIterator {
        self.undo.iter().map(Step::view)
    }

    /// The steps that are undone, nearest first: what [`Self::redo`] would bring
    /// back, in the order it would.
    pub fn undone(&self) -> impl DoubleEndedIterator<Item = StepView<'_>> + ExactSizeIterator {
        self.redo.iter().rev().map(Step::view)
    }

    /// The serial of the step on top of the undo stack, `None` at the bottom of
    /// the history: the name of the state the document is in now.
    pub fn current(&self) -> Option<u64> {
        self.undo.last().map(|s| s.serial)
    }

    /// The note of the step on top of the undo stack, for a caller completing it
    /// with what only the applied edit can tell (a created node's name).
    pub fn current_note_mut(&mut self) -> Option<&mut StepNote> {
        self.undo.last_mut().map(|s| &mut s.note)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }
}

impl Step {
    fn view(&self) -> StepView<'_> {
        StepView {
            note: &self.note,
            serial: self.serial,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::IdSource;
    use crate::node::NodeKind;
    use kurbo::{Affine, RoundedRectRadii, Size};

    /// A document with one rectangle at the origin.
    fn doc_with_rect() -> (Document, History, NodeId) {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        let mut history = History::new();
        history
            .commit(
                &mut doc,
                Transaction(vec![Operation::CreateNode {
                    id: rect,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(10.0, 10.0),
                        corner_radii: RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::IDENTITY),
                    name: None,
                }]),
            )
            .unwrap();
        (doc, history, rect)
    }

    fn move_to(id: NodeId, x: f64) -> Transaction {
        Transaction(vec![Operation::SetTransform {
            id,
            transform: Affine::translate((x, 0.0)),
        }])
    }

    fn x_of(doc: &Document, id: NodeId) -> f64 {
        doc.get(id).unwrap().transform().translation().x
    }

    /// A run of overwrites is one undo step, and undoing it lands where the run
    /// began, not one step back inside it.
    #[test]
    fn consecutive_overwrites_of_one_field_undo_as_one_step() {
        let (mut doc, mut history, rect) = doc_with_rect();
        let depth = history.undo_depth();

        for x in [1.0, 2.0, 3.0, 4.0] {
            history.commit_into_run(&mut doc, move_to(rect, x)).unwrap();
        }
        assert_eq!(x_of(&doc, rect), 4.0);
        assert_eq!(
            history.undo_depth(),
            depth + 1,
            "the four merged into one step"
        );

        history.undo(&mut doc).unwrap();
        assert_eq!(x_of(&doc, rect), 0.0, "back to before the run, not to 3");
        // And redo replays it as one step too, to where the run ended.
        history.redo(&mut doc).unwrap();
        assert_eq!(x_of(&doc, rect), 4.0);
    }

    /// After `end_run`, the next edit starts a step of its own.
    #[test]
    fn ending_a_run_separates_the_steps_either_side_of_it() {
        let (mut doc, mut history, rect) = doc_with_rect();
        history
            .commit_into_run(&mut doc, move_to(rect, 1.0))
            .unwrap();
        history.end_run();
        history
            .commit_into_run(&mut doc, move_to(rect, 2.0))
            .unwrap();

        history.undo(&mut doc).unwrap();
        assert_eq!(x_of(&doc, rect), 1.0, "only the second edit came back");
        history.undo(&mut doc).unwrap();
        assert_eq!(x_of(&doc, rect), 0.0);
    }

    /// A structural transaction never merges: folding two creates would leave the
    /// second node with no inverse on the stack.
    #[test]
    fn a_structural_edit_is_never_folded_away() {
        let (mut doc, mut history, _rect) = doc_with_rect();
        let mut ids = IdSource::new(2);
        let root = doc.first_page();
        let depth = history.undo_depth();
        for i in 0..2 {
            let id = ids.mint();
            history
                .commit_into_run(
                    &mut doc,
                    Transaction(vec![Operation::CreateNode {
                        id,
                        parent: root,
                        index: i,
                        kind: NodeKind::Rect {
                            size: Size::new(4.0, 4.0),
                            corner_radii: RoundedRectRadii::default(),
                            corner_smoothing: 0.0,
                        },
                        transform: None,
                        name: None,
                    }]),
                )
                .unwrap();
        }
        assert_eq!(history.undo_depth(), depth + 2, "two steps, not one");
        history.undo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();
        assert_eq!(doc.get(root).unwrap().children().len(), 1, "both undone");
    }

    /// A different field or node is a different edit and does not merge.
    #[test]
    fn only_the_same_fields_of_the_same_nodes_merge() {
        let (mut doc, mut history, rect) = doc_with_rect();
        let depth = history.undo_depth();
        history
            .commit_into_run(&mut doc, move_to(rect, 5.0))
            .unwrap();
        history
            .commit_into_run(
                &mut doc,
                Transaction(vec![Operation::SetOpacity {
                    id: rect,
                    opacity: 0.5,
                }]),
            )
            .unwrap();
        assert_eq!(history.undo_depth(), depth + 2);

        history.undo(&mut doc).unwrap();
        assert_eq!(doc.get(rect).unwrap().opacity(), 1.0);
        assert_eq!(x_of(&doc, rect), 5.0, "the move survived its own step");
    }

    /// A new commit drops the redo stack, so a redo never applies a stale
    /// transaction to a document that has moved on (§15 D480).
    ///
    /// flip: `self.redo.clear()` deleted from `commit` → fails at `can_redo()`;
    /// no other test in the workspace catches it.
    #[test]
    fn a_new_commit_drops_the_redo_stack() {
        let (mut doc, mut history, rect) = doc_with_rect();
        history.commit(&mut doc, move_to(rect, 7.0)).unwrap();
        history.undo(&mut doc).unwrap();
        assert!(history.can_redo(), "the fixture needs something to redo");

        history.commit(&mut doc, move_to(rect, 3.0)).unwrap();
        assert!(
            !history.can_redo(),
            "committing after an undo left {} transaction(s) on the redo stack",
            history.redo_depth()
        );
        assert!(
            history.redo(&mut doc).unwrap().is_none(),
            "and a redo press must do nothing rather than re-apply a stale step"
        );
        assert_eq!(x_of(&doc, rect), 3.0, "the document is where B put it");
    }

    /// An undo ends the run, so the edit after it is a step of its own (§15
    /// D480); otherwise B folds into C's step and is unrecoverable.
    ///
    /// C must be a different `Shape` from the run (`SetOpacity`), or A merges into
    /// C and the fixture never reaches the state.
    ///
    /// flip: `self.run = None` deleted from `undo` → fails at the first
    /// assertion. `redo`'s own line is not covered here; see the test below.
    #[test]
    fn an_undo_ends_the_run_so_the_next_edit_is_its_own_step() {
        let (mut doc, mut history, rect) = doc_with_rect();
        // C: the step a wrongly continued run would fold B into.
        history
            .commit(
                &mut doc,
                Transaction(vec![Operation::SetOpacity {
                    id: rect,
                    opacity: 0.5,
                }]),
            )
            .unwrap();
        // A, opening a run with the shape B will share.
        history
            .commit_into_run(&mut doc, move_to(rect, 2.0))
            .unwrap();
        history.undo(&mut doc).unwrap();
        let after_undo = history.undo_depth();

        history
            .commit_into_run(&mut doc, move_to(rect, 5.0))
            .unwrap();
        let depth = history.undo_depth();
        history.undo(&mut doc).unwrap();

        assert_eq!(
            x_of(&doc, rect),
            0.0,
            "one undo has to take B back; a run the undo failed to end folds B \
             into C's step and discards its inverse, so it is unrecoverable"
        );
        assert_eq!(
            doc.get(rect).unwrap().opacity(),
            0.5,
            "and it has to leave C standing — folding takes C and B in one press"
        );
        assert_eq!(
            depth,
            after_undo + 1,
            "which is the mechanism: B is a step of its own because the undo \
             cleared the run its shape would have matched"
        );
    }

    /// A redo press ends the run even when there is nothing to redo, the only case
    /// where `redo`'s `self.run = None` is reachable (§15 D480): `run` is already
    /// `None` whenever a redo pops a step.
    ///
    /// flip: `self.run = None` deleted from `redo` → fails at the depth assertion.
    #[test]
    fn a_redo_that_does_nothing_still_ends_the_run() {
        let (mut doc, mut history, rect) = doc_with_rect();
        history.commit(&mut doc, move_to(rect, 1.0)).unwrap();
        let opened = history.undo_depth();
        history
            .commit_into_run(&mut doc, move_to(rect, 2.0))
            .unwrap();
        assert_eq!(
            history.undo_depth(),
            opened,
            "the fixture needs a live run to end: B has to have merged"
        );
        assert!(
            !history.can_redo(),
            "and it needs an empty redo stack, or this exercises the other arm"
        );

        history.redo(&mut doc).unwrap();
        history
            .commit_into_run(&mut doc, move_to(rect, 5.0))
            .unwrap();

        assert_eq!(
            history.undo_depth(),
            opened + 1,
            "the redo press ended the run, so the edit after it is its own step"
        );
        history.undo(&mut doc).unwrap();
        assert_eq!(
            x_of(&doc, rect),
            2.0,
            "and one undo takes back only that edit, landing where the run left off"
        );
    }

    /// Two `SetGeometry` commits aimed at different fields are two steps (§15
    /// D482); merged, one undo would strand the radius with nothing to restore it.
    ///
    /// flip: `Operation::shape_key`'s `field` forced to `None` → fails at the
    /// depth assertion.
    #[test]
    fn two_geometry_patches_aimed_at_different_fields_do_not_merge() {
        use crate::op::GeometryPatch;
        let (mut doc, mut history, rect) = doc_with_rect();
        let depth = history.undo_depth();

        history
            .commit_into_run(
                &mut doc,
                Transaction(vec![Operation::SetGeometry {
                    id: rect,
                    geometry: GeometryPatch::Size(kurbo::Size::new(20.0, 20.0)),
                }]),
            )
            .unwrap();
        history
            .commit_into_run(
                &mut doc,
                Transaction(vec![Operation::SetGeometry {
                    id: rect,
                    geometry: GeometryPatch::CornerRadius(5.0),
                }]),
            )
            .unwrap();
        assert_eq!(
            history.undo_depth(),
            depth + 2,
            "a size edit and a radius edit write different fields, so folding one \
             into the other discards an inverse nothing else restores"
        );

        history.undo(&mut doc).unwrap();
        let NodeKind::Rect {
            size, corner_radii, ..
        } = doc.get(rect).unwrap().kind()
        else {
            panic!("the fixture is a rect");
        };
        assert_eq!(
            corner_radii.top_left, 0.0,
            "one undo takes the radius back; merged, it stayed at 5 with an empty \
             stack and no way to reach the state before the run"
        );
        assert_eq!(size.width, 20.0, "and leaves the size the first edit set");
    }

    /// A guide position can key a run, so an arrow-key nudge on a guide folds like
    /// one on a layer (§15 D482).
    ///
    /// flip: `shape_key`'s `SetGuidePosition` arm removed → fails at the depth
    /// assertion.
    #[test]
    fn a_run_of_guide_nudges_is_one_undo_step() {
        use crate::guide::{Guide, GuideAxis, GuideId};
        let (mut doc, mut history, _rect) = doc_with_rect();
        let guide = GuideId(IdSource::new(9000).mint());
        let page = doc.first_page();
        history
            .commit(
                &mut doc,
                Transaction(vec![Operation::AddGuide {
                    guide: Guide {
                        id: guide,
                        axis: GuideAxis::Horizontal,
                        position: 10.0,
                        color: None,
                        owner: page,
                    },
                }]),
            )
            .unwrap();
        let depth = history.undo_depth();

        for p in [11.0, 12.0, 13.0] {
            history
                .commit_into_run(
                    &mut doc,
                    Transaction(vec![Operation::SetGuidePosition {
                        id: guide,
                        position: p,
                    }]),
                )
                .unwrap();
        }
        assert_eq!(
            history.undo_depth(),
            depth + 1,
            "three taps of an arrow key are one nudge; each its own step is what \
             made undoing a two-second hold 45 presses"
        );

        history.undo(&mut doc).unwrap();
        assert_eq!(
            doc.guide(guide).map(|g| g.position),
            Some(10.0),
            "and one undo lands where the run began, not one tap back inside it"
        );
    }

    /// A step that fails to apply stays on its stack, for undo and for redo.
    /// flip: `undo`'s push-back on `Err` removed → fails at the depth assertion.
    #[test]
    fn a_step_that_fails_to_apply_is_kept() {
        let (mut doc, mut history, rect) = doc_with_rect();
        history.commit(&mut doc, move_to(rect, 5.0)).unwrap();
        // The rect goes outside history, so both stacks now name a missing node.
        doc.apply(&Transaction(vec![Operation::DeleteNode { id: rect }]))
            .unwrap();
        let depth = history.undo_depth();
        assert!(history.undo(&mut doc).is_err());
        assert!(history.can_undo(), "the failed step is still there");
        assert_eq!(history.undo_depth(), depth);
        assert!(!history.can_redo());

        // A fresh document: the move undone, then the rect taken away outside
        // history, so the redo fails.
        let (mut fresh, mut h2, r2) = doc_with_rect();
        h2.commit(&mut fresh, move_to(r2, 5.0)).unwrap();
        h2.undo(&mut fresh).unwrap();
        fresh
            .apply(&Transaction(vec![Operation::DeleteNode { id: r2 }]))
            .unwrap();
        assert!(h2.redo(&mut fresh).is_err());
        assert!(h2.can_redo(), "the failed redo step is still there");
    }

    fn note(label: &str) -> StepNote {
        StepNote {
            label: label.into(),
            ..StepNote::default()
        }
    }

    /// A step keeps its note and serial across undo and redo, and a run's merged
    /// edits keep the note of the edit that opened it.
    /// flip: `..step` in `undo` replaced by a default note → fails at `undone`.
    #[test]
    fn a_step_keeps_its_note_and_serial_across_undo_and_redo() {
        let (mut doc, mut history, rect) = doc_with_rect();
        history
            .commit_into_run_noted(&mut doc, move_to(rect, 1.0), note("Nudge"))
            .unwrap();
        history
            .commit_into_run_noted(&mut doc, move_to(rect, 2.0), note("Other"))
            .unwrap();
        let top = history.current().unwrap();
        assert_eq!(history.done().last().unwrap().note.label, "Nudge");

        history.undo(&mut doc).unwrap();
        let undone: Vec<_> = history
            .undone()
            .map(|s| (s.serial, s.note.label.clone()))
            .collect();
        assert_eq!(undone, vec![(top, "Nudge".to_string())]);
        assert_ne!(history.current(), Some(top));

        history.redo(&mut doc).unwrap();
        assert_eq!(history.current(), Some(top), "the same step came back");
    }

    /// `undone` lists the redo stack nearest first, the order redo takes it in.
    #[test]
    fn undone_steps_list_nearest_first() {
        let (mut doc, mut history, rect) = doc_with_rect();
        for (x, l) in [(1.0, "A"), (2.0, "B"), (3.0, "C")] {
            history
                .commit_noted(&mut doc, move_to(rect, x), note(l))
                .unwrap();
        }
        history.undo(&mut doc).unwrap();
        history.undo(&mut doc).unwrap();
        let labels: Vec<_> = history.undone().map(|s| s.note.label.as_str()).collect();
        assert_eq!(labels, ["B", "C"]);
        let done: Vec<_> = history.done().map(|s| s.note.label.as_str()).collect();
        assert_eq!(done, ["", "A"], "the fixture's create carries no note");
    }
}
