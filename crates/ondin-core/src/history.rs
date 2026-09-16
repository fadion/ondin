//! Undo/redo (§5.8).
//!
//! Because `apply` always returns the inverse, undo/redo are symmetric with no
//! special cases. History lives in core and is shared by app and MCP: one
//! timeline, into which user and AI edits interleave.
//!
//! One transaction is one undo step — except where a caller asks for a **run**
//! ([`History::commit_into_run`]), which is how a burst of arrow-key nudges
//! becomes one step instead of four. The mechanism is here and the *policy* —
//! which edits belong to one burst, and when a burst has ended — is the caller's,
//! because it is a question about gestures and keys rather than about the model.

use crate::document::Document;
use crate::id::NodeId;
use crate::op::{DirtySet, OpError, Operation, Transaction};
use std::mem::Discriminant;

/// The shape of a commit: which operation, on which node, in which order.
///
/// Two commits may be merged only if their shapes are **equal**, and that is
/// what makes keeping the older inverse sound: the second edit overwrote exactly
/// the fields the first one did, so the first inverse still restores everything
/// both of them touched. A shape is `None` for any transaction containing an
/// operation that is not a plain overwrite (`Operation::shape_key`) — a create
/// or a reparent can never be folded away, because its inverse undoes a change
/// in the tree rather than a value.
///
/// ⚠️ **The field inside the operation is part of the key, and it has to be**
/// (§15 D482, `[S2.2-L2-02]`). This was `(Discriminant<Operation>, NodeId)`, and a
/// discriminant identifies the *variant* rather than what it writes — true of the
/// field set for 22 of the 23 overwriting operations and false for `SetGeometry`,
/// whose payload is itself a field selector. `Operation::shape_key` carries the
/// patch's discriminant for exactly that one, and has the measurement.
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

#[derive(Default)]
pub struct History {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// The shape of the most recent commit, while it is still open to being
    /// merged into. Cleared by anything that ends a run: an undo, a redo, a
    /// commit that is not part of one, or the caller saying so.
    run: Option<Shape>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply `tx`, push its inverse to the undo stack, and clear the redo stack.
    /// On error the document is untouched (atomic `apply`) and history unchanged.
    /// Returns the `DirtySet` so the caller can update its `Resolved` (§9.2).
    ///
    /// **Ends any run in progress**, so an unrelated edit landing between two
    /// nudges keeps them apart. The commit itself becomes the run a following
    /// [`Self::commit_into_run`] may merge into — that is how the *first* edit of
    /// a burst gets extended by the second without the caller having to know
    /// which one it is on.
    pub fn commit(&mut self, doc: &mut Document, tx: Transaction) -> Result<DirtySet, OpError> {
        let shape = shape(&tx);
        let outcome = doc.apply(&tx)?;
        self.undo.push(outcome.inverse);
        self.redo.clear();
        self.run = shape;
        Ok(outcome.dirty)
    }

    /// Apply `tx` and fold it into the previous undo step instead of pushing one
    /// of its own — when that is sound, and plain [`Self::commit`] when it is not.
    ///
    /// **The merge keeps the older inverse and drops the new one.** Two
    /// consecutive absolute overwrites of the same field compose: `A` then `B`
    /// leaves the state `B` describes, and the inverse of `A` restores what was
    /// there before either — so a single undo of the pair is exactly `A`'s own
    /// inverse, already sitting on the stack. That argument needs both edits to
    /// have overwritten the *same* fields of the *same* nodes, which is what
    /// [`Shape`] checks; anything else falls through to a fresh step rather than
    /// silently losing an operation.
    ///
    /// The caller decides *when* a run continues (see the module docs). This only
    /// refuses the merges that would be wrong.
    pub fn commit_into_run(
        &mut self,
        doc: &mut Document,
        tx: Transaction,
    ) -> Result<DirtySet, OpError> {
        let shape = shape(&tx);
        if shape.is_none() || shape != self.run || !self.can_undo() {
            return self.commit(doc, tx);
        }
        // The step already on the stack stays, and this edit's own inverse is
        // discarded: it would restore the state *this* edit found, which is where
        // the previous one left off and not where the run began.
        let outcome = doc.apply(&tx)?;
        self.redo.clear();
        Ok(outcome.dirty)
    }

    /// End the current run, so the next [`Self::commit_into_run`] starts a step
    /// of its own.
    ///
    /// The caller's half of the policy: a gesture finishing, a different verb, a
    /// selection changing, a pause long enough to read as a second decision.
    pub fn end_run(&mut self) {
        self.run = None;
    }

    /// Undo the most recent transaction, returning its `DirtySet` (or `None` if
    /// there was nothing to undo). The inverse is pushed to the redo stack.
    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<DirtySet>, OpError> {
        self.run = None;
        let Some(tx) = self.undo.pop() else {
            return Ok(None);
        };
        let outcome = doc.apply(&tx)?;
        self.redo.push(outcome.inverse);
        Ok(Some(outcome.dirty))
    }

    /// Redo the most recently undone transaction, returning its `DirtySet` (or
    /// `None` if there was nothing to redo).
    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<DirtySet>, OpError> {
        // ⚠️ **Before the `pop`, and that placement is the whole of what this line
        // does** (§15 D480). A redo that actually restores a step always finds
        // `run` already `None` — the stack is non-empty only if nothing has been
        // committed since the last undo or redo, and both of those end the run —
        // so the only reachable effect is that a redo press with *nothing to redo*
        // ends the run in progress. That is what [`Self::run`]'s doc means by
        // listing a redo among the things that end one, and
        // `a_redo_that_does_nothing_still_ends_the_run` is where it is pinned.
        self.run = None;
        let Some(tx) = self.redo.pop() else {
            return Ok(None);
        };
        let outcome = doc.apply(&tx)?;
        self.undo.push(outcome.inverse);
        Ok(Some(outcome.dirty))
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

    /// **A run of overwrites is one undo step, and undoing it lands where the run
    /// began** — not one step back inside it. Four nudges being four undos is the
    /// symptom this exists for.
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

    /// The caller ends a run, and the next edit starts a step of its own — the
    /// policy half (a gesture finishing, a pause, a different verb) lives up
    /// there, so this is the only thing history has to get right.
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

    /// **A structural transaction never merges, however it is committed.** The
    /// merge keeps the older inverse and throws the newer one away, and the
    /// inverse of a create is a delete: fold two creates together and the second
    /// node is left in the document with nothing on the stack that removes it.
    #[test]
    fn a_structural_edit_is_never_folded_away() {
        let (mut doc, mut history, _rect) = doc_with_rect();
        let mut ids = IdSource::new(2);
        let root = doc.root();
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

    /// A different field, or a different node, is a different edit — merging
    /// those would leave the first one's inverse standing for both and lose one.
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

    /// **A new commit drops the redo stack**, so a redo can never apply a
    /// transaction to a document that has moved on since it was undone
    /// (§15 D480).
    ///
    /// The textbook redo bug, and until this test the line holding it back was
    /// asserted by nothing in the workspace. Without `redo.clear()`: commit A,
    /// undo it, commit B — the stack still holds the transaction that redoes A,
    /// and pressing redo applies it against a document B has since changed. For a
    /// `SetTransform` that silently re-writes a stale value **and pushes its own
    /// inverse onto the undo stack**, so history ends up containing a step the
    /// user never made.
    ///
    /// **Two assertions and the second is the one with teeth.** `can_redo()` is
    /// the flag; `redo()` returning `None` is the behaviour, and a redo stack
    /// emptied by anything other than this line would still fail it. The
    /// document's own x is asserted last because it is what the user would see:
    /// a redo that fires here moves the rectangle back to A's position.
    ///
    /// ⚠️ **Flip-check, run: `self.redo.clear()` deleted from `commit`.** Fails on
    /// `can_redo()`, which is the predicted site. ⚠️ It fails **only here** — the
    /// whole workspace was green under that deletion before this test existed
    /// (`[S2.2-L6-04]`, flipped against `cargo test --workspace`, not a subset),
    /// including `tests/spine.rs`'s `undo_redo_roundtrip…`, which walks the stack
    /// to empty without ever committing in between and so cannot reach the state
    /// at all.
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

    /// **An undo ends the run, so the edit after it is a step of its own**
    /// (§15 D480) — the second line in this file that nothing asserted.
    ///
    /// `commit_into_run` merges when the new edit has the same `Shape` as the
    /// run in progress. Without `run = None` in `undo`, an undo leaves that shape
    /// standing: commit C, start a run with A, undo, then commit B with A's shape
    /// — and B folds into **C's** step, discarding its own inverse. One undo then
    /// reverts C *and* B in a single press, with B unrecoverable.
    ///
    /// **The loss is asserted before the mechanism**: what a reader is shown on
    /// failure is that one press reverted an edit it should not have and left one
    /// standing that it should have taken, and the step count is the *why*
    /// underneath it.
    ///
    /// ⚠️ **C has to be a different `Shape` from the run**, or the fixture never
    /// reaches the state this test is named for. A first draft made C another
    /// `SetTransform`, so A merged into C immediately and the undo two lines later
    /// took *both* back — leaving nothing on the stack for B to fold into and the
    /// flip green for a reason that had nothing to do with the line. It is a
    /// `SetOpacity`, and the opacity assertion below is the one that says C
    /// survived.
    ///
    /// ⚠️ **Flip-check, run: `self.run = None` deleted from `undo`.** Fails on the
    /// **first** assertion — the rectangle still at 5.0 after an undo, i.e. B
    /// applied and unrecoverable — which is the predicted site, and the opacity and
    /// depth assertions would both have failed behind it. Workspace-green without
    /// this test.
    ///
    /// ⚠️ **`redo` carries the same line and this does not cover it** — it is a
    /// separate statement in a separate function, and deleting it leaves this test
    /// green. Its own mirror is two tests below.
    #[test]
    fn an_undo_ends_the_run_so_the_next_edit_is_its_own_step() {
        let (mut doc, mut history, rect) = doc_with_rect();
        // C — the step a wrongly-continued run would fold B into. A different
        // shape from the moves, so it is a step of its own and stays one.
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

    /// **A redo press ends the run even when there is nothing to redo** — and that
    /// is the *only* way `redo`'s `self.run = None` is reachable (§15 D480).
    ///
    /// ⚠️ **The obvious mirror of the test above is vacuous, and a flip is what
    /// said so.** Written the natural way — commit, undo, redo, then an edge
    /// sharing the restored step's shape — the flip left every test green, because
    /// **`run` is already `None` at every redo that actually pops something.** The
    /// argument is short and worth keeping: the redo stack is non-empty only if
    /// nothing has been committed since the last undo or redo (both `commit` and
    /// `commit_into_run` clear it), and `undo` and `redo` both end the run. So a
    /// redo that restores a step finds `run` already cleared, and the line is doing
    /// nothing there.
    ///
    /// What is left is the empty-stack case, because **the line sits before the
    /// `pop`**: commit A, extend it with B, press redo with nothing to redo, and
    /// the next edit of the same shape is a step of its own instead of folding into
    /// the run. That is *intended* rather than incidental — `History::run`'s own
    /// doc lists "a redo" among the things that end a run — but it is a decision
    /// nothing recorded and nothing could have caught being reversed.
    ///
    /// ⚠️ **Flip-check, run: `self.run = None` deleted from `redo`.** Fails on the
    /// depth assertion at 2 against 3 — the predicted site — with every other test
    /// in this module green.
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

    /// **Two `SetGeometry` commits aimed at different fields are two steps**
    /// (§15 D482, `[S2.2-L2-02]`).
    ///
    /// `Self::commit_into_run`'s doc states the soundness condition and claims
    /// this code enforces it — *"both edits to have overwritten the **same** fields
    /// of the **same** nodes, which is what `Shape` checks"* — and that was false
    /// for exactly one operation. A `Discriminant<Operation>` names the *variant*,
    /// and `SetGeometry`'s payload is itself a field selector with twelve arms. So
    /// a size edit and a corner-radius edit on one rect merged, the second's
    /// inverse was discarded, and **one undo restored the size and stranded the
    /// radius with an empty stack** — a state that was never committed and nothing
    /// left to reach the real previous one.
    ///
    /// **The strand is asserted, not just the depth.** A depth of 2 says the merge
    /// was refused; the radius coming back says what the merge was costing. The
    /// depth assertion is kept in front of it because it is the mechanism and it
    /// fails first, which is the right order here — the loss below it is only
    /// reachable *through* the wrong depth.
    ///
    /// ⚠️ **Flip-check, run: `Operation::shape_key`'s `field` forced to `None`**,
    /// which is the key this had before. Fails on the depth assertion at 1 against
    /// 2 — the predicted site — and the radius assertion behind it would have
    /// failed too.
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

    /// **A guide position can key a run**, which is what lets an arrow-key nudge on
    /// a guide fold the way the same keystroke on a layer always has (§15 D482,
    /// `[S2.2-L2-03]`).
    ///
    /// `Operation::overwrites` answers `None` for every guide operation, and until
    /// D482 `Shape` was built from it — so `SetGuidePosition` could never fold
    /// however it was committed, and `rulers::nudge_guides` called plain `commit`.
    /// A two-second hold of `↓` is about **45** entries on the undo stack at the
    /// Windows repeat rate, against one for the identical keystroke on a layer.
    ///
    /// ⚠️ **Asserted here rather than in `rulers.rs` because the obstruction was
    /// core's**, not the app's: the app arm was a one-line change and this is the
    /// line that made it possible. `nudge_guides`' own call is covered by
    /// construction — one `commit_run` with nothing between it and the transaction.
    ///
    /// ⚠️ **Flip-check, run: `shape_key`'s `SetGuidePosition` arm removed**, so it
    /// falls through to `overwrites()?` and answers `None`. Fails on the depth
    /// assertion at **5 against 3** — the predicted site, though not the predicted
    /// numbers: the fixture's own two commits sit under the run, so "three taps,
    /// three steps" reads as 5 rather than as 3. Every other test in this module
    /// stays green.
    #[test]
    fn a_run_of_guide_nudges_is_one_undo_step() {
        use crate::guide::{Guide, GuideAxis, GuideId};
        let (mut doc, mut history, _rect) = doc_with_rect();
        let guide = GuideId(IdSource::new(9000).mint());
        history
            .commit(
                &mut doc,
                Transaction(vec![Operation::AddGuide {
                    guide: Guide {
                        id: guide,
                        axis: GuideAxis::Horizontal,
                        position: 10.0,
                        color: None,
                        owner: None,
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
}
