//! M1 spine: the load-bearing invariants of the mutation path.
//!
//! Proves atomic transactions, symmetric undo/redo that restores identical
//! `NodeId`s and identical state, and the core validation rules — for the first
//! operations (create/delete/insert-subtree/set-transform). If these hold for
//! this handful of ops, the remaining ops are mechanical variations.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::{Document, History, IdSource, NodeId, NodeKind, OpError, Operation, Transaction};

fn artboard() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(800.0, 600.0),
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(100.0, 50.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

/// A document with one artboard under root; returns (doc, id_source, artboard_id).
fn doc_with_artboard() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x1234);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: artboard(),
        transform: None,
        name: None,
    }]))
    .expect("create artboard");
    (doc, ids, ab)
}

#[test]
fn create_builds_expected_tree() {
    let (mut doc, mut ids, ab) = doc_with_artboard();
    let root = doc.root();

    let r = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: r,
        parent: ab,
        index: 0,
        kind: rect(),
        transform: None,
        name: None,
    }]))
    .unwrap();

    assert_eq!(doc.get(root).unwrap().children(), &[ab]);
    assert_eq!(doc.get(ab).unwrap().children(), &[r]);
    assert_eq!(doc.get(r).unwrap().parent(), Some(ab));
    // `name: None` → the kind's name, numbered within the parent (§15 D127). Every
    // generated name carries a number, the first one included.
    assert_eq!(doc.get(r).unwrap().name(), "Rectangle 1");
    assert_eq!(doc.len(), 3);
}

#[test]
fn transaction_is_atomic_on_failure() {
    let (mut doc, mut ids, ab) = doc_with_artboard();
    let before = doc.clone();

    // Two ops: a valid create, then a create with a duplicate id. The whole
    // transaction must roll back — including the first, valid op.
    let good = ids.mint();
    let dup = ids.mint();
    let tx = Transaction(vec![
        Operation::CreateNode {
            id: good,
            parent: ab,
            index: 0,
            kind: rect(),
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: dup,
            parent: ab,
            index: 0,
            kind: rect(),
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: dup, // duplicate id within the same transaction -> DuplicateId
            parent: ab,
            index: 0,
            kind: rect(),
            transform: None,
            name: None,
        },
    ]);

    let err = doc.apply(&tx).unwrap_err();
    assert!(matches!(err, OpError::DuplicateId(_)));
    assert_eq!(doc, before, "a failed transaction must mutate nothing");
    assert!(
        !doc.contains(good),
        "the earlier valid op must be rolled back"
    );
}

#[test]
fn undo_redo_roundtrip_restores_identical_state_and_ids() {
    let (mut doc, mut ids, ab) = doc_with_artboard();
    let mut hist = History::new();

    // Two committed edits: create a rect, then move it.
    let r = ids.mint();
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::CreateNode {
            id: r,
            parent: ab,
            index: 0,
            kind: rect(),
            transform: None,
            name: None,
        }]),
    )
    .unwrap();
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetTransform {
            id: r,
            transform: Affine::translate((30.0, 40.0)),
        }]),
    )
    .unwrap();

    let full = doc.clone();
    assert_eq!(
        doc.get(r).unwrap().transform(),
        Affine::translate((30.0, 40.0))
    );

    // Undo everything.
    assert!(hist.undo(&mut doc).unwrap().is_some()); // undo the move
    assert!(hist.undo(&mut doc).unwrap().is_some()); // undo the create
    assert!(!doc.contains(r), "rect should be gone after undo");

    // Redo everything.
    assert!(hist.redo(&mut doc).unwrap().is_some()); // redo the create
    assert!(hist.redo(&mut doc).unwrap().is_some()); // redo the move
    assert!(
        hist.redo(&mut doc).unwrap().is_none(),
        "nothing left to redo"
    );

    assert_eq!(doc, full, "redo must restore identical state");
    assert!(
        doc.contains(r),
        "the SAME NodeId must be restored, not a new one"
    );
    assert_eq!(
        doc.get(r).unwrap().transform(),
        Affine::translate((30.0, 40.0))
    );
}

#[test]
fn delete_then_undo_restores_identical_subtree() {
    let (mut doc, mut ids, ab) = doc_with_artboard();
    let mut hist = History::new();

    // Build a small subtree under the artboard: a group with two rects.
    let group = ids.mint();
    let r1 = ids.mint();
    let r2 = ids.mint();
    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: r1,
                parent: group,
                index: 0,
                kind: rect(),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: r2,
                parent: group,
                index: 1,
                kind: rect(),
                transform: None,
                name: None,
            },
        ]),
    )
    .unwrap();

    let with_group = doc.clone();

    // Delete the group (and its whole subtree), then undo.
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::DeleteNode { id: group }]),
    )
    .unwrap();
    assert!(!doc.contains(group) && !doc.contains(r1) && !doc.contains(r2));

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert_eq!(
        doc, with_group,
        "undo of delete must restore the identical subtree"
    );
    // Identical ids, and z-order preserved.
    assert_eq!(doc.get(group).unwrap().children(), &[r1, r2]);
}

#[test]
fn validation_rejects_structural_violations() {
    let (mut doc, mut ids, ab) = doc_with_artboard();
    let root = doc.root();

    // An artboard inside an artboard is *allowed* (§5.3, relaxed): a frame is a
    // page, and a page inside a page is a card, a component, a state. It used to be
    // `ArtboardNesting`.
    let nested = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: nested,
        parent: ab,
        index: 0,
        kind: artboard(),
        transform: None,
        name: None,
    }]))
    .expect("frames nest");
    assert_eq!(doc.get(nested).unwrap().parent(), Some(ab));

    // What a frame still may not do is hang off a group, which has no box of its own
    // for a page to be clipped by.
    let grp = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: grp,
        parent: ab,
        index: 0,
        kind: NodeKind::Group,
        transform: None,
        name: None,
    }]))
    .expect("a group under a frame");
    let in_group = ids.mint();
    let err = doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: in_group,
            parent: grp,
            index: 0,
            kind: artboard(),
            transform: None,
            name: None,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::ArtboardPlacement));

    // A non-artboard directly under the root is *allowed* (§5.3, relaxed): a
    // shape dragged more than half out of its frame has to land somewhere, and
    // "no frame" is the honest answer. It used to be `InvalidParent`.
    let stray = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: stray,
        parent: root,
        index: 0,
        kind: rect(),
        transform: None,
        name: None,
    }]))
    .expect("the root may hold loose shapes");
    assert_eq!(doc.get(stray).unwrap().parent(), Some(root));

    // Creating another Root.
    let bad_root = ids.mint();
    let err = doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: bad_root,
            parent: ab,
            index: 0,
            kind: NodeKind::Root,
            transform: None,
            name: None,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WrongKindForOp));

    // Index past the end of the child list.
    let oob = ids.mint();
    let err = doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: oob,
            parent: ab,
            index: 5,
            kind: rect(),
            transform: None,
            name: None,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::IndexOutOfRange));

    // Deleting the root.
    let err = doc
        .apply(&Transaction(vec![Operation::DeleteNode { id: root }]))
        .unwrap_err();
    assert!(matches!(err, OpError::CannotModifyRoot));

    // Operating on an unknown node.
    let ghost = ids.mint();
    let err = doc
        .apply(&Transaction(vec![Operation::SetTransform {
            id: ghost,
            transform: Affine::IDENTITY,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::NoSuchNode(_)));

    // Opacity out of range.
    let err = doc
        .apply(&Transaction(vec![Operation::SetOpacity {
            id: ab,
            opacity: 1.5,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::BadOpacity));

    // Every *failure* left the document alone: what is here is the root, the
    // artboard, and the three the relaxed rules legitimately allowed — a frame
    // inside the frame, a group inside it, and one loose shape at the root.
    assert_eq!(doc.len(), 5);
}
