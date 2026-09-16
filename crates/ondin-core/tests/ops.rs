//! Coverage for the remaining M1 operations: reparent (incl. cycle rejection),
//! reorder, and the geometry/text/paint setters — each with kind validation and
//! undo/redo identity.

use ondin_core::Brush;
use ondin_core::kurbo::{Point, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Effect, EffectKind, Fill, GeometryPatch, Guide, GuideAxis, GuideId, History,
    IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, NodeId, NodeKind, OpError, Operation,
    Stroke, StrokeAlign, TextSizing, TextStyle, Transaction, image_brush,
};

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

fn text() -> NodeKind {
    NodeKind::Text {
        content: "hi".into(),
        style: Box::new(text_style(12.0)),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn text_style(size: f64) -> TextStyle {
    TextStyle {
        font_family: "Inter".into(),
        font_size: size,
        weight: 400,
        italic: false,
        line_height: Some(ondin_core::Length::Em(1.2)),
        ..Default::default()
    }
}

fn solid(r: u8, g: u8, b: u8) -> Fill {
    Fill {
        brush: Brush::Solid(Color::from_rgba8(r, g, b, 255)),
        visible: true,
    }
}

/// Create a node under `parent` at the end, committing through history.
fn create(hist: &mut History, doc: &mut Document, id: NodeId, parent: NodeId, kind: NodeKind) {
    let index = doc.get(parent).unwrap().children().len();
    hist.commit(
        doc,
        Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: None,
            name: None,
        }]),
    )
    .unwrap();
}

/// doc with root→artboard; returns (doc, ids, artboard_id).
fn base() -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0x7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let mut hist = History::new();
    create(&mut hist, &mut doc, ab, root, artboard());
    (doc, ids, ab)
}

#[test]
fn reparent_moves_subtree_and_undo_restores() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let group = ids.mint();
    let r = ids.mint();
    create(&mut hist, &mut doc, group, ab, NodeKind::Group);
    create(&mut hist, &mut doc, r, ab, rect());

    let before = doc.clone();
    // Move r from the artboard into the group.
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::Reparent {
            id: r,
            new_parent: group,
            index: 0,
        }]),
    )
    .unwrap();
    assert_eq!(doc.get(r).unwrap().parent(), Some(group));
    assert_eq!(doc.get(group).unwrap().children(), &[r]);
    assert_eq!(doc.get(ab).unwrap().children(), &[group]);

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert_eq!(doc, before, "undo of reparent restores exact prior state");
}

#[test]
fn reparent_rejects_cycles() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let group = ids.mint();
    let child = ids.mint();
    create(&mut hist, &mut doc, group, ab, NodeKind::Group);
    create(&mut hist, &mut doc, child, group, NodeKind::Group);

    // Into itself.
    let err = doc
        .apply(&Transaction(vec![Operation::Reparent {
            id: group,
            new_parent: group,
            index: 0,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WouldCycle));

    // Into its own descendant.
    let err = doc
        .apply(&Transaction(vec![Operation::Reparent {
            id: group,
            new_parent: child,
            index: 0,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WouldCycle));
}

#[test]
fn reorder_within_parent_and_undo() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let a = ids.mint();
    let b = ids.mint();
    let c = ids.mint();
    create(&mut hist, &mut doc, a, ab, rect());
    create(&mut hist, &mut doc, b, ab, rect());
    create(&mut hist, &mut doc, c, ab, rect());
    assert_eq!(doc.get(ab).unwrap().children(), &[a, b, c]);

    // Move c to the front.
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::Reorder { id: c, index: 0 }]),
    )
    .unwrap();
    assert_eq!(doc.get(ab).unwrap().children(), &[c, a, b]);

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert_eq!(doc.get(ab).unwrap().children(), &[a, b, c]);
}

#[test]
fn set_geometry_applies_and_reverses() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let r = ids.mint();
    create(&mut hist, &mut doc, r, ab, rect());

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetGeometry {
            id: r,
            geometry: GeometryPatch::Size(Size::new(200.0, 80.0)),
        }]),
    )
    .unwrap();
    assert_eq!(
        doc.get(r).unwrap().kind(),
        &NodeKind::Rect {
            size: Size::new(200.0, 80.0),
            corner_radii: RoundedRectRadii::default()
        }
    );

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert_eq!(
        doc.get(r).unwrap().kind(),
        &NodeKind::Rect {
            size: Size::new(100.0, 50.0),
            corner_radii: RoundedRectRadii::default()
        }
    );
}

#[test]
fn set_geometry_rejects_mismatched_kind() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let r = ids.mint();
    create(&mut hist, &mut doc, r, ab, rect());

    // LineEnd patch on a Rect.
    let err = doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: r,
            geometry: GeometryPatch::LineEnd(Point::new(1.0, 1.0)),
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WrongKindForOp));
}

#[test]
fn set_text_and_style_on_text_only() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let t = ids.mint();
    let r = ids.mint();
    create(&mut hist, &mut doc, t, ab, text());
    create(&mut hist, &mut doc, r, ab, rect());

    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::SetText {
                id: t,
                content: "hello world".into(),
                spans: Default::default(),
                para_spans: Default::default(),
            },
            Operation::SetTextStyle {
                id: t,
                style: text_style(24.0),
                spans: None,
            },
        ]),
    )
    .unwrap();
    let NodeKind::Text { content, style, .. } = doc.get(t).unwrap().kind() else {
        panic!("expected text node");
    };
    assert_eq!(content, "hello world");
    assert_eq!(style.font_size, 24.0);

    // Text ops on a non-text node are rejected.
    let err = doc
        .apply(&Transaction(vec![Operation::SetText {
            id: r,
            content: "nope".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WrongKindForOp));

    // Undo restores original content+style.
    assert!(hist.undo(&mut doc).unwrap().is_some());
    let NodeKind::Text { content, style, .. } = doc.get(t).unwrap().kind() else {
        panic!();
    };
    assert_eq!(content, "hi");
    assert_eq!(style.font_size, 12.0);
}

/// **The paragraph overrides ride on `SetText`, so its inverse owes them too.**
/// They are byte ranges into the content, which is why they travel with it rather
/// than on their own — and an inverse that restored the old string with the new
/// ranges would leave every override describing somebody else's paragraph.
#[test]
fn undoing_set_text_restores_the_paragraph_spans_with_the_content() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let t = ids.mint();
    create(&mut hist, &mut doc, t, ab, text());

    let paragraph = ondin_core::ParagraphStyle::default();
    let mut before = ondin_core::ParaSpans::default();
    before.set(
        0..2,
        ondin_core::ParaAttr::IndentStart(ondin_core::Length::Px(12.0)),
        &paragraph,
    );
    let mut after = ondin_core::ParaSpans::default();
    after.set(
        0..5,
        ondin_core::ParaAttr::IndentStart(ondin_core::Length::Px(30.0)),
        &paragraph,
    );

    // The fixture's content is "hi", so the first list is in range for it.
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetParagraphSpans {
            id: t,
            spans: before.clone(),
        }]),
    )
    .unwrap();
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetText {
            id: t,
            content: "hello".into(),
            spans: Default::default(),
            para_spans: after.clone(),
        }]),
    )
    .unwrap();

    let NodeKind::Text { para_spans, .. } = doc.get(t).unwrap().kind() else {
        panic!("expected text node");
    };
    assert_eq!(*para_spans, after);

    assert!(hist.undo(&mut doc).unwrap().is_some());
    let NodeKind::Text {
        content,
        para_spans,
        ..
    } = doc.get(t).unwrap().kind()
    else {
        panic!();
    };
    assert_eq!(content, "hi");
    assert_eq!(*para_spans, before);
}

/// **A span dropped as redundant comes back on undo, in both scopes** (§15 D163).
///
/// Setting the node's *defaults* re-states the override list against them, because
/// a span equal to the old default was never stored — so an override that has just
/// become an echo of the new default is correctly and invisibly dropped. The
/// inverse used to carry the defaults alone, which made that drop one-way: node
/// size 12, a span puts one run at 24, set the node to 24 and undo it, and the run
/// went to 12 with everything else rather than back to its own 24.
///
/// Both scopes, in one test, because they are one mechanism: `op_set_paragraph_style`
/// re-states for the same reason and inherited the same defect.
#[test]
fn undo_restores_a_span_the_defaults_made_redundant() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let t = ids.mint();
    create(&mut hist, &mut doc, t, ab, text());

    // The character scope: one run of "hi" set to 24 while the node is 12.
    let small = text_style(12.0);
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..1, ondin_core::CharAttr::Size(24.0), &small);
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetTextSpans {
            id: t,
            spans: spans.clone(),
        }]),
    )
    .unwrap();

    // The paragraph scope: the same shape, one paragraph asking for an indent the
    // node does not have.
    let flat = ondin_core::ParagraphStyle::default();
    let indent = ondin_core::Length::Px(10.0);
    let mut para_spans = ondin_core::ParaSpans::default();
    para_spans.set(0..1, ondin_core::ParaAttr::IndentStart(indent), &flat);
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetParagraphSpans {
            id: t,
            spans: para_spans.clone(),
        }]),
    )
    .unwrap();

    // Now the defaults move to exactly what those two overrides were asking for.
    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::SetTextStyle {
                id: t,
                style: text_style(24.0),
                spans: None,
            },
            Operation::SetParagraphStyle {
                id: t,
                paragraph: ondin_core::ParagraphStyle {
                    indent_start: indent,
                    ..flat.clone()
                },
                spans: None,
            },
        ]),
    )
    .unwrap();

    let NodeKind::Text {
        spans: got,
        para_spans: got_para,
        ..
    } = doc.get(t).unwrap().kind()
    else {
        panic!("expected text node");
    };
    assert!(
        got.is_empty() && got_para.is_empty(),
        "both overrides are now echoes of the defaults and are dropped: \
         {got:?} / {got_para:?}"
    );

    assert!(hist.undo(&mut doc).unwrap().is_some());
    let NodeKind::Text {
        style,
        spans: got,
        paragraph,
        para_spans: got_para,
        ..
    } = doc.get(t).unwrap().kind()
    else {
        panic!("expected text node");
    };
    assert_eq!(style.font_size, 12.0, "the defaults come back");
    assert_eq!(paragraph.indent_start, ondin_core::Length::Px(0.0));
    assert_eq!(
        *got, spans,
        "and so does the 24pt run, rather than flattening to the node's 12"
    );
    assert_eq!(
        *got_para, para_spans,
        "and so does the indented paragraph, rather than rejoining the node's 0"
    );

    // Redo installs the dropped lists explicitly rather than re-deriving them, so
    // the pair round-trips indefinitely.
    assert!(hist.redo(&mut doc).unwrap().is_some());
    let NodeKind::Text {
        spans: got,
        para_spans: got_para,
        ..
    } = doc.get(t).unwrap().kind()
    else {
        panic!("expected text node");
    };
    assert!(
        got.is_empty() && got_para.is_empty(),
        "{got:?} / {got_para:?}"
    );
}

#[test]
fn set_fills_and_strokes_paintable_only() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let g = ids.mint();
    let r = ids.mint();
    create(&mut hist, &mut doc, g, ab, NodeKind::Group);
    create(&mut hist, &mut doc, r, ab, rect());

    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::SetFills {
                id: r,
                fills: vec![solid(200, 30, 30)],
            },
            Operation::SetStrokes {
                id: r,
                strokes: vec![Stroke {
                    brush: Brush::Solid(Color::BLACK),
                    width: 1.0,
                    join: ondin_core::kurbo::Join::Miter,
                    cap: ondin_core::kurbo::Cap::Butt,
                    dashes: vec![],
                    dash_offset: 0.0,
                    sides: Default::default(),
                    miter_limit: 4.0,
                    dash_fit: false,
                    align: StrokeAlign::Center,
                    visible: true,
                }],
            },
        ]),
    )
    .unwrap();
    assert_eq!(doc.get(r).unwrap().paint().fills.len(), 1);
    assert_eq!(doc.get(r).unwrap().paint().strokes.len(), 1);

    // Fills on a group are rejected.
    let err = doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: g,
            fills: vec![solid(0, 0, 0)],
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WrongKindForOp));

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert!(doc.get(r).unwrap().paint().fills.is_empty());
}

/// **`SetFills` reaches a frame, and undoes off one.**
///
/// ⚠️ **This test was `set_artboard_background_artboard_only` and its subject was
/// the opposite fact** (§15 D400). It committed a `SetArtboardBackground`, checked
/// the same op on a *rect* came back `WrongKindForOp`, and undid. Two thirds of
/// that no longer exist: the op is gone, and there is no kind a fill op accepts
/// that its twin refuses — `is_paintable` is one list for both, and
/// `set_fills_and_strokes_undo` above already pins the kinds it *rejects*.
///
/// What is left is the half that changed and is worth an assertion of its own: the
/// op layer accepts a frame at all. Written against `is_paintable` rather than
/// against the panel, because that gate is what used to answer `false` here and
/// would take the whole feature with it if it went back.
///
/// Flipped by removing `NodeKind::Artboard` from `takes_paint`: the commit fails
/// with `WrongKindForOp` before any assertion is reached.
#[test]
fn set_fills_reaches_a_frame() {
    let (mut doc, _ids, ab) = base();
    let mut hist = History::new();
    let ground = solid(250, 250, 250);

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetFills {
            id: ab,
            fills: vec![ground.clone(), solid(0, 0, 255)],
        }]),
    )
    .unwrap();
    // Two, because "a frame holds one ground" is exactly the rule that went away.
    assert_eq!(doc.get(ab).unwrap().paint().fills.len(), 2);
    assert_eq!(doc.get(ab).unwrap().paint().fills[0], ground);

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert!(doc.get(ab).unwrap().paint().fills.is_empty());
}

/// The proportion lock is a document edit like any other, so it has to undo
/// like one — and it starts off, on every kind, since a new shape is free to be
/// dragged into whatever proportions the user wants.
#[test]
fn set_proportions_locked_applies_and_reverses() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let r = ids.mint();
    create(&mut hist, &mut doc, r, ab, rect());
    assert!(!doc.get(r).unwrap().proportions_locked());

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetProportionsLocked {
            id: r,
            locked: true,
        }]),
    )
    .unwrap();
    assert!(doc.get(r).unwrap().proportions_locked());
    // And not by confusing itself with the layer lock, which is a different
    // field with a nearly identical name.
    assert!(!doc.get(r).unwrap().locked());

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert!(!doc.get(r).unwrap().proportions_locked());
    assert!(hist.redo(&mut doc).unwrap().is_some());
    assert!(doc.get(r).unwrap().proportions_locked());
}

/// **A duplicate is the same layer with different ids** (`[S2.1-L6-03]`,
/// §15 D502).
///
/// This asserted ids and shape and **nothing about content**. `remap_subtree`
/// writes a `Node` literal enumerating nineteen fields by hand, sixteen of them
/// copied one at a time, and the fixture set none of them — so an
/// id-and-shape assertion was satisfied by any implementation that got the graph
/// right, *including one that defaulted every payload*. Changing
/// `exports: n.exports.clone()` to `exports: Vec::new()` — a duplicated icon set
/// up to write 1× and 2× PNGs coming back writing nothing — left all 1,856
/// workspace tests green.
///
/// Three of the sixteen carry a **documented argument** in `remap_subtree` for
/// being copied at all (`exports`, `effects`, `grids`), and all three were
/// enforced by nothing. That is this project's second vacuity shape — *an
/// assertion the wrong version also satisfies*.
///
/// **Flip run**, `exports: Vec::new()`: fails on *"Grp: exports"*, naming the
/// field, which is the point of comparing field by field rather than whole
/// `Node`s. It was the predicted site — the loop reaches `exports` before
/// `effects` and `grids`, so it is also the only one of the three the flip can
/// reach, and each would have to be flipped separately to say the same of them.
#[test]
fn capture_remap_insert_duplicates_a_subtree_with_fresh_ids() {
    use ondin_core::remap_subtree;
    use std::collections::HashSet;

    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let mut hist = History::new();
    let ab = ids.mint();
    let group = ids.mint();
    let r1 = ids.mint();
    let r2 = ids.mint();
    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: artboard(),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: group,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: Some("Grp".into()),
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

    // **Give the subtree something to lose** (`[S2.1-L6-03]`, §15 D502).
    // `remap_subtree` writes a `Node` literal enumerating nineteen fields by
    // hand, sixteen of them copied one at a time, and until this fixture existed
    // the template carried a default in every one of them — so the assertions
    // below could not tell a copy from a fresh node with the right shape.
    // `exports: Vec::new()` in `remap_subtree`, i.e. a duplicated icon silently
    // losing the export settings that made it worth duplicating, left all 1,856
    // workspace tests green.
    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::SetOpacity {
                id: group,
                opacity: 0.5,
            },
            Operation::SetLocked {
                id: group,
                locked: true,
            },
            Operation::SetProportionsLocked {
                id: group,
                locked: true,
            },
            Operation::SetPivot {
                id: group,
                pivot: Some(ondin_core::Pivot::Normalized(ondin_core::kurbo::Vec2::new(
                    0.25, 0.75,
                ))),
            },
            Operation::SetEffects {
                id: group,
                effects: vec![Effect {
                    kind: EffectKind::DropShadow(ondin_core::Shadow {
                        blur: 4.0,
                        spread: 1.0,
                        ..ondin_core::Shadow::default()
                    }),
                    visible: true,
                }],
            },
            Operation::SetExports {
                id: group,
                exports: vec![ondin_core::ExportSpec::new(
                    ondin_core::ExportFormat::Png,
                    ondin_core::ExportScale::Times(2.0),
                )],
            },
            Operation::SetLayoutGrids {
                id: group,
                grids: vec![ondin_core::LayoutGrid::new(ondin_core::GridAxis::Columns)],
            },
            Operation::SetVisible {
                id: r1,
                visible: false,
            },
            Operation::SetMask { id: r1, mask: true },
            Operation::SetMaskMode {
                id: r1,
                mode: ondin_core::MaskMode::Alpha,
            },
            Operation::SetName {
                id: r1,
                name: "Cutter".into(),
            },
            Operation::SetFills {
                id: r1,
                fills: vec![Fill {
                    brush: Brush::Solid(Color::from_rgba8(10, 20, 30, 255)),
                    visible: true,
                }],
            },
            Operation::SetClip { id: r2, clip: true },
            Operation::SetFillRule {
                id: r2,
                rule: ondin_core::FillRule::EvenOdd,
            },
            Operation::SetTransform {
                id: r2,
                transform: ondin_core::kurbo::Affine::translate((7.0, 11.0)),
            },
            Operation::SetGeometry {
                id: r2,
                geometry: GeometryPatch::CornerRadius(3.0),
            },
        ]),
    )
    .unwrap();

    // Capture the group subtree (group + 2 rects), root first.
    let template = doc.capture_subtree(group).unwrap();
    assert_eq!(template.len(), 3);
    assert_eq!(template[0].id(), group);

    let (nodes, new_root) = remap_subtree(&template, &mut ids).unwrap();
    // Fresh ids: none of the new ids collide with the originals.
    let originals: HashSet<NodeId> = [group, r1, r2].into_iter().collect();
    for n in &nodes {
        assert!(!originals.contains(&n.id()), "id was not remapped");
    }
    // Structure preserved: new root has two children, all internal to the set.
    let new_set: HashSet<NodeId> = nodes.iter().map(|n| n.id()).collect();
    let root_node = nodes.iter().find(|n| n.id() == new_root).unwrap();
    assert_eq!(root_node.children().len(), 2);
    for n in &nodes {
        if n.id() != new_root {
            assert!(new_set.contains(&n.parent().unwrap()));
        }
    }

    // **Every field but the three structural ones**, in template order — the
    // capture is root-first and `remap_subtree` preserves it, so the pairing is
    // positional and asserted as such before it is relied on. Written out rather
    // than compared as whole `Node`s so a failure names the field that was
    // dropped; three of them (`exports`, `effects`, `grids`) carry a *documented
    // argument* in `remap_subtree` for being copied at all, and all three were
    // enforced by nothing.
    for (old, new) in template.iter().zip(nodes.iter()) {
        let at = old.name();
        assert_eq!(old.kind(), new.kind(), "{at}: kind");
        assert_eq!(old.transform(), new.transform(), "{at}: transform");
        assert_eq!(old.name(), new.name(), "{at}: name");
        assert_eq!(old.visible(), new.visible(), "{at}: visible");
        assert_eq!(old.locked(), new.locked(), "{at}: locked");
        assert_eq!(
            old.proportions_locked(),
            new.proportions_locked(),
            "{at}: proportions_locked"
        );
        assert_eq!(old.opacity(), new.opacity(), "{at}: opacity");
        assert_eq!(old.clip(), new.clip(), "{at}: clip");
        assert_eq!(old.mask(), new.mask(), "{at}: mask");
        assert_eq!(old.mask_mode(), new.mask_mode(), "{at}: mask_mode");
        assert_eq!(old.fill_rule(), new.fill_rule(), "{at}: fill_rule");
        assert_eq!(old.paint(), new.paint(), "{at}: paint");
        assert_eq!(old.pivot(), new.pivot(), "{at}: pivot");
        assert_eq!(old.exports(), new.exports(), "{at}: exports");
        assert_eq!(old.effects(), new.effects(), "{at}: effects");
        assert_eq!(old.grids(), new.grids(), "{at}: grids");
    }
    // And the fixture is the thing that gives those assertions teeth, so it says
    // so out loud rather than being trusted: a default-valued template makes the
    // whole loop above vacuous.
    let root_template = &template[0];
    assert!(
        !root_template.exports().is_empty()
            && !root_template.effects().is_empty()
            && !root_template.grids().is_empty()
            && root_template.pivot().is_some()
            && root_template.opacity() != 1.0,
        "the fixture must carry a non-default value in every field with an argument"
    );
    assert!(
        template.iter().any(|n| n.mask())
            && template.iter().any(|n| n.clip())
            && template.iter().any(|n| !n.visible())
            && template.iter().any(|n| !n.paint().fills.is_empty()),
        "and the leaves must carry the rest of them"
    );

    // Insert as a sibling under the artboard, then undo restores the count.
    let before = template_node_count(&doc);
    let dirty = hist
        .commit(
            &mut doc,
            Transaction(vec![Operation::InsertSubtree {
                nodes,
                parent: ab,
                index: 1,
            }]),
        )
        .unwrap();
    assert!(dirty.0.contains(&new_root));
    assert_eq!(template_node_count(&doc), before + 3);
    assert!(doc.contains(new_root));

    assert!(hist.undo(&mut doc).unwrap().is_some());
    assert_eq!(template_node_count(&doc), before);
    assert!(!doc.contains(new_root));
}

fn template_node_count(doc: &Document) -> usize {
    // Count reachable nodes from the root.
    let mut count = 0;
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        if let Some(n) = doc.get(id) {
            count += 1;
            stack.extend(n.children().iter().copied());
        }
    }
    count
}

// --- guides ---------------------------------------------------------------

fn guide_at(ids: &mut IdSource, axis: GuideAxis, position: f64) -> Guide {
    Guide {
        id: GuideId(ids.mint()),
        axis,
        position,
        color: None,
        owner: None,
    }
}

/// The five guide operations, each inverted by its own undo. `RemoveGuide`
/// inverts to the *whole* guide, so a recoloured one comes back recoloured
/// rather than as a fresh default in the same place.
#[test]
fn guide_operations_round_trip_through_undo() {
    let mut ids = IdSource::new(0x6D1DE);
    let mut doc = Document::new(ids.mint());
    let mut hist = History::new();
    let red = Color::from_rgba8(200, 30, 60, 255);

    let guide = guide_at(&mut ids, GuideAxis::Horizontal, 332.0);
    hist.commit(&mut doc, Transaction(vec![Operation::AddGuide { guide }]))
        .unwrap();
    assert_eq!(doc.guides(), [guide]);

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetGuidePosition {
            id: guide.id,
            position: 12.5,
        }]),
    )
    .unwrap();
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetGuideColor {
            id: guide.id,
            color: Some(red),
        }]),
    )
    .unwrap();
    let moved = *doc.guide(guide.id).unwrap();
    assert_eq!((moved.position, moved.color), (12.5, Some(red)));

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::RemoveGuide { id: guide.id }]),
    )
    .unwrap();
    assert!(doc.guides().is_empty());

    // Undo the removal: the guide comes back as it was when it left.
    hist.undo(&mut doc).unwrap();
    assert_eq!(doc.guide(guide.id), Some(&moved));
    hist.undo(&mut doc).unwrap();
    assert_eq!(doc.guide(guide.id).unwrap().color, None);
    hist.undo(&mut doc).unwrap();
    assert_eq!(doc.guide(guide.id).unwrap().position, 332.0);
    hist.undo(&mut doc).unwrap();
    assert!(doc.guides().is_empty());
}

/// Removing every guide is one transaction, so it is one undo step — the
/// inspector's "Remove all Guides" must not cost N presses of Ctrl+Z.
#[test]
fn removing_all_guides_is_a_single_undo_step() {
    let mut ids = IdSource::new(0x6D1DE);
    let mut doc = Document::new(ids.mint());
    let mut hist = History::new();

    let placed: Vec<Guide> = (0..4)
        .map(|i| guide_at(&mut ids, GuideAxis::Vertical, i as f64 * 100.0))
        .collect();
    hist.commit(
        &mut doc,
        Transaction(
            placed
                .iter()
                .map(|g| Operation::AddGuide { guide: *g })
                .collect(),
        ),
    )
    .unwrap();
    assert_eq!(doc.guides().len(), 4);

    let remove_all: Vec<Operation> = doc
        .guides()
        .iter()
        .map(|g| Operation::RemoveGuide { id: g.id })
        .collect();
    hist.commit(&mut doc, Transaction(remove_all)).unwrap();
    assert!(doc.guides().is_empty());

    hist.undo(&mut doc).unwrap();
    let mut back: Vec<Guide> = doc.guides().to_vec();
    back.sort_by_key(|g| g.id);
    assert_eq!(back, placed);
}

/// A guide op naming an id the document does not have is an error, and — since
/// `apply` validates against a working copy — it takes the whole transaction
/// down with it rather than leaving half of it applied.
#[test]
fn a_guide_op_on_a_missing_guide_fails_atomically() {
    let mut ids = IdSource::new(0x6D1DE);
    let mut doc = Document::new(ids.mint());
    let good = guide_at(&mut ids, GuideAxis::Horizontal, 10.0);
    let ghost = GuideId(ids.mint());

    let err = doc
        .apply(&Transaction(vec![
            Operation::AddGuide { guide: good },
            Operation::SetGuidePosition {
                id: ghost,
                position: 0.0,
            },
        ]))
        .unwrap_err();
    assert!(matches!(err, OpError::NoSuchGuide(id) if id == ghost));
    assert!(doc.guides().is_empty(), "the good half must not have stuck");

    doc.apply(&Transaction(vec![Operation::AddGuide { guide: good }]))
        .unwrap();
    let err = doc
        .apply(&Transaction(vec![Operation::AddGuide { guide: good }]))
        .unwrap_err();
    assert!(matches!(err, OpError::DuplicateGuide(id) if id == good.id));
}

// --- guide scope (§5.5, the `owner` field) ---------------------------------

/// A document with a frame in it, and a second frame nested inside the first.
fn framed() -> (Document, IdSource, NodeId, NodeId) {
    let mut ids = IdSource::new(0x6D1DE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let outer = ids.mint();
    let inner = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: outer,
            parent: root,
            index: 0,
            kind: artboard(),
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: inner,
            parent: outer,
            index: 0,
            kind: artboard(),
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    (doc, ids, outer, inner)
}

/// Rescoping moves the owner and the position **together**, and inverts to the
/// old pair — so a guide dragged into a frame and undone is back at the world
/// coordinate it had, not at the frame-local number reinterpreted as one.
#[test]
fn rescoping_a_guide_moves_its_owner_and_position_as_one_step() {
    let (mut doc, mut ids, frame, _) = framed();
    let mut hist = History::new();

    // A global guide at world y = 340.
    let guide = guide_at(&mut ids, GuideAxis::Horizontal, 340.0);
    hist.commit(&mut doc, Transaction(vec![Operation::AddGuide { guide }]))
        .unwrap();

    // Dragged into a frame whose origin is at world y = 300: the same line, now
    // 40 down the frame.
    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetGuideScope {
            id: guide.id,
            owner: Some(frame),
            position: 40.0,
        }]),
    )
    .unwrap();
    let scoped = *doc.guide(guide.id).unwrap();
    assert_eq!((scoped.owner, scoped.position), (Some(frame), 40.0));

    // One undo, and both halves come back. A `SetGuidePosition` beside a
    // scope-only setter would have needed two.
    hist.undo(&mut doc).unwrap();
    let back = *doc.guide(guide.id).unwrap();
    assert_eq!(
        (back.owner, back.position),
        (None, 340.0),
        "the guide must return to the space it came from, not keep the local number"
    );

    hist.redo(&mut doc).unwrap();
    assert_eq!(*doc.guide(guide.id).unwrap(), scoped);
}

/// A guide may only be scoped to a live **frame**. Neither a shape, nor a group,
/// nor the root, nor a node that is not there at all — a scoped guide's position
/// is read in its owner's space, so an owner with no box is a guide with no
/// place.
#[test]
fn a_guide_may_only_be_scoped_to_a_frame() {
    let (mut doc, mut ids, frame, _) = framed();
    let shape = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: shape,
        parent: frame,
        index: 0,
        kind: rect(),
        transform: None,
        name: None,
    }]))
    .unwrap();
    let root = doc.root();
    let ghost = ids.mint();

    for (name, owner) in [
        ("a shape", shape),
        ("the root", root),
        ("a node that is not there", ghost),
    ] {
        let mut guide = guide_at(&mut ids, GuideAxis::Vertical, 10.0);
        guide.owner = Some(owner);
        let err = doc
            .apply(&Transaction(vec![Operation::AddGuide { guide }]))
            .unwrap_err();
        assert!(
            matches!(err, OpError::BadGuideOwner(id) if id == owner),
            "{name} was accepted as a guide's scope: {err:?}"
        );
        assert!(doc.guides().is_empty(), "{name} left a guide behind");
    }

    // And the frame itself is fine, nested one included.
    let mut guide = guide_at(&mut ids, GuideAxis::Vertical, 10.0);
    guide.owner = Some(frame);
    doc.apply(&Transaction(vec![Operation::AddGuide { guide }]))
        .unwrap();
    assert_eq!(doc.guide(guide.id).unwrap().owner, Some(frame));

    // A rescope onto an illegal owner is refused too, and leaves the guide alone.
    let err = doc
        .apply(&Transaction(vec![Operation::SetGuideScope {
            id: guide.id,
            owner: Some(shape),
            position: 0.0,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::BadGuideOwner(id) if id == shape));
    assert_eq!(doc.guide(guide.id).unwrap().owner, Some(frame));
}

/// `guides_of` collects the guides a delete must take with it — including those
/// scoped to a **nested** frame inside the doomed subtree, which is the half a
/// direct `owner == id` filter would miss.
#[test]
fn deleting_a_frame_collects_the_guides_of_every_frame_inside_it() {
    let (mut doc, mut ids, outer, inner) = framed();
    let elsewhere = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: elsewhere,
        parent: doc.root(),
        index: 1,
        kind: artboard(),
        transform: None,
        name: None,
    }]))
    .unwrap();

    let mut on_outer = guide_at(&mut ids, GuideAxis::Horizontal, 10.0);
    on_outer.owner = Some(outer);
    let mut on_inner = guide_at(&mut ids, GuideAxis::Vertical, 20.0);
    on_inner.owner = Some(inner);
    let mut on_other = guide_at(&mut ids, GuideAxis::Vertical, 30.0);
    on_other.owner = Some(elsewhere);
    let global = guide_at(&mut ids, GuideAxis::Horizontal, 40.0);
    doc.apply(&Transaction(
        [on_outer, on_inner, on_other, global]
            .into_iter()
            .map(|guide| Operation::AddGuide { guide })
            .collect(),
    ))
    .unwrap();

    let ops = ondin_core::build::guides_of(&doc, &[outer]);
    let taken: Vec<GuideId> = ops
        .iter()
        .map(|op| match op {
            Operation::RemoveGuide { id } => *id,
            other => panic!("guides_of produced {other:?}"),
        })
        .collect();
    let mut want = vec![on_outer.id, on_inner.id];
    want.sort();
    assert_eq!(
        taken, want,
        "the nested frame's guide travels with its host"
    );
}

/// **An export scale that cannot be a size is refused by the model, and dropped
/// by the loader** (§15 D492, `[S8.2-L1-03]`).
///
/// `op_set_exports` was six lines and bounded nothing. A `.ondin` carrying
/// `"scale": {"Width": 0}` loaded, and `ondin export --all` wrote **`Button@0w.png`
/// at 1 × 1** — exit 0, no warning, `clamped == false`. The filename asserts a
/// 0-pixel width and the asset is junk a build script will copy. `Times(-2.0)` is
/// the same shape: `factor` floors it to `1e-4` and the file is named `@-2x`.
///
/// ⚠️ **Both other entry points already refused exactly this set, and said why.**
/// `ExportScale::from_label` filters `*px > 0` and `n.is_finite() && *n > 0.0`, and
/// the CLI's parse rejects the same values *"rather than left to `factor`'s floor,
/// which exists to keep the renderer safe and **would turn `--scale 0` into a
/// one-pixel file nobody asked for**"* — which is precisely the file this made. The
/// comment describing the outcome sat on the two paths that could not reach it, and
/// the model was the only door without the guard.
///
/// **The two halves answer differently on purpose.** An operation is refused,
/// because the app should never author one; a *file* has its unusable specs
/// **dropped**, because refusing the whole document over an export preset would
/// lose the artwork to save the recipe, and `Width(0)` has no defensible non-zero
/// reading to repair it to.
///
/// ⚠️ **The valid cases are the control**, and they matter more than usual here:
/// the predicate is three comparisons and getting one backwards would refuse every
/// export in the app with all the same tests green otherwise.
#[test]
fn an_export_scale_that_cannot_be_a_size_is_refused() {
    use ondin_core::export::{ExportFormat, ExportScale, ExportSpec};

    let (mut doc, _, frame, _) = framed();
    let spec = |scale| ExportSpec::new(ExportFormat::Png, scale);
    let set = |doc: &mut Document, scale| {
        doc.apply(&Transaction(vec![Operation::SetExports {
            id: frame,
            exports: vec![spec(scale)],
        }]))
    };

    for bad in [
        ExportScale::Width(0),
        ExportScale::Height(0),
        ExportScale::Times(0.0),
        ExportScale::Times(-2.0),
        ExportScale::Times(f32::NAN),
        ExportScale::Times(f32::INFINITY),
    ] {
        assert!(
            matches!(set(&mut doc, bad), Err(OpError::BadExportSpec)),
            "{bad:?} cannot be a size and must not reach the document"
        );
    }
    for good in [
        ExportScale::Times(1.0),
        ExportScale::Times(0.5),
        ExportScale::Width(512),
        ExportScale::Height(1),
    ] {
        assert!(
            set(&mut doc, good).is_ok(),
            "{good:?} is an ordinary export and must still work"
        );
    }

    // And a file that carries one loses the row rather than the document.
    //
    // ⚠️ **Hand-edited JSON, because the model now refuses to build one.** That is
    // the point of the two halves being different, and it is also the only way to
    // reach the loader's arm at all — a test-only setter on `Document` would be a
    // door into the model that exists solely to break it.
    doc.apply(&Transaction(vec![Operation::SetExports {
        id: frame,
        exports: vec![spec(ExportScale::Width(512)), spec(ExportScale::Times(2.0))],
    }]))
    .expect("two ordinary export rows");
    let bytes = ondin_core::io::save(&doc).expect("it saves");
    let json = String::from_utf8(bytes).expect("the format is JSON");
    assert!(
        json.contains("512"),
        "the fixture has to reach the file, or the edit below finds nothing"
    );
    let edited = json.replacen("512", "0", 1);
    let back = ondin_core::io::load(edited.as_bytes()).expect("the document still opens");
    assert_eq!(
        back.get(frame).map(|n| n.exports().len()),
        Some(1),
        "the unusable spec is dropped and the usable one survives"
    );
}

/// **Deleting a frame without its guides is refused, rather than producing a
/// document that never opens again** (§15 D491, `[S2.2-L2-05]`).
///
/// `Document::check_guide_owner` states the rule — *"a guide may only be scoped to
/// a frame that is in the document"* — and enforced it on `AddGuide` and
/// `SetGuideScope`, the two operations that **name** an owner. There is a second
/// way to break it, which is to remove the frame, and `op_delete` never looked at
/// `self.guides` at all. Measured through `Document::apply` alone, no
/// hand-editing: the delete returned `Ok`, `doc.guides()` kept the orphan, and
/// `io::load(io::save(&doc))` refused the bytes **forever** — *"guide 51:4 is
/// scoped to 51:2, which is not a frame in this document"*. Autosave writes that
/// file, and the in-memory session that could still undo it is the only copy of
/// the user's work.
///
/// ⚠️ **There was no live bug, and that is why this is a post-condition and not a
/// patch.** Every production `DeleteNode` construction site was enumerated: there
/// are **seven**, and exactly **one** — `app::delete_selection` — can name an
/// `Artboard`, which is the one that carries `build::guides_of`. None of the other
/// six can name one at all: `build::outline` refuses a frame through
/// `build::can_outline`, `flatten_union` and `text_on_new_path` refuse it by kind (§15
/// D453), `ungroup` deletes a group, and the canvas's two delete a text node.
///
/// ⚠️ **This paragraph said "both that can name an `Artboard`" and there is one**
/// — corrected by `arch-scribe` counting the sites rather than reading the claim.
/// The conclusion is unchanged and in fact stronger: what existed was an invariant
/// enforced by a convention **one** caller knows about, which a second would have
/// to be told. So the check is at the end of `apply`,
/// where the document becomes real, and catches the next op that breaks the rule
/// as well as this one.
///
/// **The round trip is the assertion, not the error type.** What the finding is
/// about is a file that will not load; asserting `Err(BadGuideOwner)` would pin
/// the mechanism and let a future spelling that corrupts differently through.
///
/// ⚠️ **The `guides_of` order is the control**, and it has to stay green: this
/// refusal is worthless if it also refuses the correct spelling. The test above
/// covers the undo; this covers the save.
#[test]
fn deleting_a_frame_that_owns_a_guide_is_refused_or_still_loads() {
    let (mut doc, mut ids, frame, _) = framed();
    let mut guide = guide_at(&mut ids, GuideAxis::Horizontal, 40.0);
    guide.owner = Some(frame);
    doc.apply(&Transaction(vec![Operation::AddGuide { guide }]))
        .unwrap();

    let mut bare = doc.clone();
    let refused = bare.apply(&Transaction(vec![Operation::DeleteNode { id: frame }]));
    if refused.is_ok() {
        let bytes = ondin_core::io::save(&bare).expect("it saves");
        assert!(
            ondin_core::io::load(&bytes).is_ok(),
            "a delete that leaves an orphaned guide has to be refused, or the \
             document it makes has to still open — it did neither"
        );
    }

    // The control: the spelling `build::guides_of` produces still works.
    let mut ops = ondin_core::build::guides_of(&doc, &[frame]);
    ops.push(Operation::DeleteNode { id: frame });
    let mut right = doc.clone();
    right
        .apply(&Transaction(ops))
        .expect("the correct spelling must not be caught by this guard");
    let bytes = ondin_core::io::save(&right).expect("it saves");
    assert!(ondin_core::io::load(&bytes).is_ok(), "and reloads");
}

/// **The order inside a delete transaction is load-bearing.** `apply` inverts a
/// transaction by reversing it, so the guides have to be removed *before* the
/// frame for the undo to put the frame back *before* the guide — otherwise
/// `AddGuide` arrives while its owner is still missing and the whole undo fails.
///
/// Written as the two orders side by side, because the wrong one deletes
/// perfectly well and only breaks Ctrl+Z.
#[test]
fn undoing_a_frame_delete_restores_its_guides() {
    let (mut doc, mut ids, frame, _) = framed();
    let mut guide = guide_at(&mut ids, GuideAxis::Horizontal, 40.0);
    guide.owner = Some(frame);
    doc.apply(&Transaction(vec![Operation::AddGuide { guide }]))
        .unwrap();

    // The order `guides_of`'s contract asks for.
    let mut ops = ondin_core::build::guides_of(&doc, &[frame]);
    ops.push(Operation::DeleteNode { id: frame });
    let mut hist = History::new();
    let mut right = doc.clone();
    hist.commit(&mut right, Transaction(ops.clone())).unwrap();
    assert!(right.guides().is_empty());
    assert!(!right.contains(frame));
    hist.undo(&mut right).unwrap();
    assert_eq!(
        right.guide(guide.id),
        Some(&guide),
        "the guide must come back scoped to the frame that came back with it"
    );

    // And the order that reads just as naturally and cannot be undone.
    let mut wrong = vec![Operation::DeleteNode { id: frame }];
    wrong.extend(ondin_core::build::guides_of(&doc, &[frame]));
    let mut hist = History::new();
    let mut broken = doc.clone();
    hist.commit(&mut broken, Transaction(wrong)).unwrap();
    assert!(
        hist.undo(&mut broken).is_err(),
        "nodes-then-guides has to fail its own undo — if this ever passes, \
         `guides_of`'s ordering contract has stopped mattering and its \
         documentation is wrong"
    );
}

// --- the image table (§5.5a) ------------------------------------------------

fn jpeg_bytes() -> Vec<u8> {
    // Not a real JPEG, and it does not need to be: core never decodes. What it
    // has to be is *bytes with a zero in them*, because that is what a text
    // format has to survive and what base64 is here for.
    vec![0xFF, 0xD8, 0x00, 0x01, 0x7F, 0x80, 0xFE, 0x00]
}

fn photo() -> (ImageId, ImageEntry) {
    (
        ImageId("sha256:beef".into()),
        ImageEntry {
            source: ImageSource::Embedded(jpeg_bytes().into()),
            format: ImageFormat::Jpeg,
            width: 2000,
            height: 1500,
        },
    )
}

/// **A committed edit does not copy the photographs** (§15 D301).
///
/// `Document::apply` clones the whole document to get a working copy it can fail
/// out of, so for as long as `ImageSource::Embedded` held a `Vec<u8>` every commit
/// — and every undo and every redo, which go through the same function — deep-copied
/// every embedded image. Measured before the fix, in release, for a one-transform
/// nudge: **3.5 ms at 20 MB of images and 10.2 ms at 60 MB**, against 0.001 ms
/// after. A held arrow key commits per repeat, so it was a stutter with no visible
/// cause and no upper bound but the document's size.
///
/// **Asserted as identity rather than as a time**, because the timing is the
/// symptom and the sharing is the fact: the same allocation has to be reachable
/// from both documents. A stopwatch here would be a flaky test of this machine.
///
/// The pointer is taken through `bytes()`, the accessor everything else reads the
/// table with, so the test would still hold if the variant's inner type changed
/// again — what it pins is that a clone shares, not that it is an `Arc`.
#[test]
fn cloning_a_document_shares_its_image_bytes_rather_than_copying_them() {
    let mut doc = Document::new(IdSource::new(1).mint());
    let (id, entry) = photo();
    doc.apply(&Transaction(vec![Operation::AddImage {
        id: id.clone(),
        entry,
    }]))
    .expect("add the photo");

    let original = doc.image(&id).and_then(|e| e.bytes()).expect("bytes");
    // **The control, and it is not a formality**: `<[u8]>::as_ptr` on an *empty*
    // slice answers a dangling constant, so two empty slices compare equal and
    // every assertion below would hold against a deep copy. This says the fixture
    // has bytes and that a real copy of them lands somewhere else.
    assert!(!original.is_empty(), "an empty fixture proves nothing here");
    // **Bound to a name, not `original.to_vec().as_ptr()`.** That spelling drops the
    // `Vec` at the end of the statement and compares a *dangling* pointer, which the
    // allocator is free to hand straight back — so the control could pass or fail for
    // reasons that have nothing to do with the thing it is controlling for. Caught by
    // `dangling_pointers_from_temporaries` the day `--all-targets` joined the clippy
    // gate (§15 D302), in a test written the day before.
    let independent = original.to_vec();
    assert_ne!(
        independent.as_ptr(),
        original.as_ptr(),
        "a genuine copy has a different address, so the test below can fail"
    );

    let copy = doc.clone();
    let cloned = copy.image(&id).and_then(|e| e.bytes()).expect("bytes");
    assert_eq!(original, cloned, "a clone still holds the same bytes");
    assert_eq!(
        original.as_ptr(),
        cloned.as_ptr(),
        "and it must hold the *same allocation* — a deep copy here is paid on \
         every commit, undo and redo"
    );

    // And through `apply` itself, which is where the clone actually happens and
    // the only reason any of this matters.
    let mut doc2 = doc.clone();
    doc2.apply(&Transaction(vec![Operation::SetCanvasBackground {
        background: Color::WHITE,
    }]))
    .expect("any committed edit");
    assert_eq!(
        doc2.image(&id)
            .and_then(|e| e.bytes())
            .expect("bytes")
            .as_ptr(),
        original.as_ptr(),
        "a commit's working copy shares the table too"
    );
}

/// **Undoing a purge brings the bytes back.**
///
/// The reason `AddImage` carries the whole entry rather than an id: nothing else
/// in the document is holding an image's bytes, so an inverse that named only the
/// key would restore a reference to nothing — a layer that survives undo and is
/// empty for ever. The image plan named this as one of its traps ("undo has to
/// restore the table entry too") and §5.5a states it as the rule — "deleting the
/// only layer showing an image and undoing must bring the *bytes* back". It is a
/// trap precisely because the *node* half of the undo works perfectly while the
/// picture is gone.
#[test]
fn undoing_a_removed_image_restores_its_bytes() {
    let mut ids = IdSource::new(1);
    let mut doc = Document::new(ids.mint());
    let mut hist = History::new();
    let (id, entry) = photo();

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::AddImage {
            id: id.clone(),
            entry: entry.clone(),
        }]),
    )
    .unwrap();
    assert_eq!(doc.image(&id), Some(&entry));

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::RemoveImage { id: id.clone() }]),
    )
    .unwrap();
    assert_eq!(doc.image(&id), None, "the purge took it out");

    hist.undo(&mut doc).unwrap();
    assert_eq!(
        doc.image(&id).and_then(ImageEntry::bytes),
        Some(jpeg_bytes().as_slice()),
        "undo restored the entry but not the bytes"
    );

    // And the round trip closes: redo takes it out again.
    hist.redo(&mut doc).unwrap();
    assert_eq!(doc.image(&id), None);
}

/// **One id, one entry** — and the error is about that, not about placing the
/// same picture twice.
///
/// Placing a file twice must be free: the caller hashes the bytes, finds the id
/// already there, and adds a second *reference*. Reaching `DuplicateImage` means
/// two different entries claimed one key, which is a bug in whatever minted it.
#[test]
fn an_image_id_cannot_be_claimed_twice_and_an_absent_one_cannot_be_removed() {
    let mut ids = IdSource::new(1);
    let mut doc = Document::new(ids.mint());
    let (id, entry) = photo();

    doc.apply(&Transaction(vec![Operation::AddImage {
        id: id.clone(),
        entry: entry.clone(),
    }]))
    .unwrap();

    let again = doc.apply(&Transaction(vec![Operation::AddImage {
        id: id.clone(),
        entry,
    }]));
    assert!(
        matches!(again, Err(OpError::DuplicateImage(_))),
        "{again:?}"
    );

    let ghost = doc.apply(&Transaction(vec![Operation::RemoveImage {
        id: ImageId("sha256:nothing".into()),
    }]));
    assert!(matches!(ghost, Err(OpError::NoSuchImage(_))), "{ghost:?}");

    // The failed pair changed nothing: a transaction applies atomically.
    assert_eq!(doc.image_count(), 1);
}

/// **Deleting the layer does not delete the picture.**
///
/// Nothing collects the table, and this is the case that rule exists for: the
/// delete and the undo of it are about the *node*, and if the bytes had gone with
/// it there would be nothing for the restored fill to point at. The cost — a
/// document can carry an image nothing shows — is what *Purge unused* is for.
#[test]
fn deleting_the_last_layer_showing_an_image_leaves_the_table_alone() {
    let mut ids = IdSource::new(1);
    let mut doc = Document::new(ids.mint());
    let root = doc.root();
    let mut hist = History::new();
    let (id, entry) = photo();
    let rect = ids.mint();

    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::AddImage {
                id: id.clone(),
                entry,
            },
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(200.0, 150.0),
                    corner_radii: RoundedRectRadii::from_single_radius(0.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetFills {
                id: rect,
                fills: vec![Fill {
                    brush: image_brush(id.clone()),
                    visible: true,
                }],
            },
        ]),
    )
    .unwrap();

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::DeleteNode { id: rect }]),
    )
    .unwrap();
    assert!(!doc.contains(rect), "the layer went");
    assert!(
        doc.has_image(&id),
        "the bytes must outlive the only layer that showed them"
    );

    hist.undo(&mut doc).unwrap();
    let fills = &doc.get(rect).unwrap().paint().fills;
    assert_eq!(
        fills[0].brush,
        image_brush(id),
        "the restored layer points at the image it always did"
    );
}

/// A mask governs the run above it up to the **next** mask, and a mask is never
/// itself masked.
///
/// Both halves are asserted, and each is answered by code the other is not.
/// Scanning *forward* for the first mask in the parent passes every case here
/// except `d`, whose run belongs to the second mask; and dropping the "a mask
/// begins a run" rule passes every case except `m2`, which would come back
/// clipped by `m1` and so draw its own siblings through two clips instead of one.
#[test]
fn a_mask_governs_up_to_the_next_one_and_is_never_masked_itself() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    // Bottom-first, which is what `create` appending gives: a, m1, b, c, m2, d.
    let named: Vec<NodeId> = (0..6).map(|_| ids.mint()).collect();
    for id in &named {
        create(&mut hist, &mut doc, *id, ab, rect());
    }
    let (a, m1, b, c, m2, d) = (named[0], named[1], named[2], named[3], named[4], named[5]);
    assert_eq!(
        doc.get(ab).unwrap().children(),
        &named[..],
        "the fixture is in the order the assertions below read"
    );
    for m in [m1, m2] {
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::SetMask { id: m, mask: true }]),
        )
        .unwrap();
    }

    assert_eq!(doc.governing_mask(a), None, "nothing below a to mask it");
    assert_eq!(doc.governing_mask(b), Some(m1));
    assert_eq!(doc.governing_mask(c), Some(m1));
    assert_eq!(
        doc.governing_mask(d),
        Some(m2),
        "the nearer mask owns the run, not the first one in the list"
    );
    assert_eq!(doc.governing_mask(m1), None, "a mask begins a run");
    assert_eq!(
        doc.governing_mask(m2),
        None,
        "and so does the second one, rather than being clipped by the first"
    );
}

/// The kinds that cannot be a mask are refused at the op layer rather than left
/// inert, because the scene walk has to decide whether the layer paints and a
/// flag with no answer must not reach it.
#[test]
fn a_mask_is_refused_on_the_kinds_that_cannot_be_one() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let root = doc.root();
    let shapes = [rect(), text(), NodeKind::Group];
    for kind in shapes {
        let id = ids.mint();
        create(&mut hist, &mut doc, id, ab, kind);
        hist.commit(
            &mut doc,
            Transaction(vec![Operation::SetMask { id, mask: true }]),
        )
        .expect("a shape, a text layer and a group can all mask");
        assert!(doc.get(id).unwrap().mask());
    }
    for id in [ab, root] {
        assert!(
            matches!(
                doc.apply(&Transaction(vec![Operation::SetMask { id, mask: true }])),
                Err(OpError::WrongKindForOp)
            ),
            "a frame is a page and the root is not a layer"
        );
        assert!(!doc.get(id).unwrap().mask(), "and neither of them took it");
    }
}

/// **Releasing is the inverse, not a second verb.** A mask draws nothing, so the
/// canvas cannot offer a way back and undo has to be exact.
#[test]
fn masking_undoes_to_the_flag_it_replaced() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let r = ids.mint();
    create(&mut hist, &mut doc, r, ab, rect());
    assert!(!doc.get(r).unwrap().mask());

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetMask { id: r, mask: true }]),
    )
    .unwrap();
    assert!(doc.get(r).unwrap().mask());
    hist.undo(&mut doc).unwrap();
    assert!(!doc.get(r).unwrap().mask(), "undo took the mask off");
    hist.redo(&mut doc).unwrap();
    assert!(doc.get(r).unwrap().mask(), "redo put it back");
}

/// **The mode outlives the mask being switched off**, which is the whole reason
/// it is a field beside the flag rather than a payload inside it.
///
/// The gesture this protects is ordinary: set a mask to Alpha, release it to look
/// at what is underneath, put it back. With the mode inside the flag there is
/// nowhere for `Alpha` to live while the mask is off, so the release quietly
/// throws it away and the toggle turns out to have been destructive.
///
/// Also asserts the mode is writable while the layer is **not** a mask, which is
/// the same fact from the other side and is what makes `SetMaskMode` its own
/// operation.
#[test]
fn the_mask_mode_survives_the_mask_being_released() {
    use ondin_core::MaskMode;
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let r = ids.mint();
    create(&mut hist, &mut doc, r, ab, rect());
    assert_eq!(
        doc.get(r).unwrap().mask_mode(),
        MaskMode::Shape,
        "fixture: a fresh layer masks by its outline"
    );

    hist.commit(
        &mut doc,
        Transaction(vec![
            Operation::SetMask { id: r, mask: true },
            Operation::SetMaskMode {
                id: r,
                mode: MaskMode::Alpha,
            },
        ]),
    )
    .unwrap();
    assert_eq!(doc.get(r).unwrap().mask_mode(), MaskMode::Alpha);

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetMask { id: r, mask: false }]),
    )
    .unwrap();
    assert!(!doc.get(r).unwrap().mask(), "released");
    assert_eq!(
        doc.get(r).unwrap().mask_mode(),
        MaskMode::Alpha,
        "and the mode is still there to come back to"
    );

    hist.commit(
        &mut doc,
        Transaction(vec![Operation::SetMask { id: r, mask: true }]),
    )
    .unwrap();
    assert_eq!(
        doc.get(r).unwrap().mask_mode(),
        MaskMode::Alpha,
        "so putting the mask back puts back the mask the user had"
    );
}

/// Effects take a **container** and refuse only the **root**, which is the
/// opposite gate from every other paint operation and the reason `SetEffects`
/// does not reuse `is_paintable`.
///
/// Both halves are asserted, because each fails on its own. A gate copied from
/// `SetFills` refuses the group; no gate at all accepts the root, and an effect
/// on the root is an effect on the document — a blur over the workspace itself,
/// with no layer it could belong to and nothing in the panel that could show it.
#[test]
fn effects_go_on_a_container_but_never_on_the_root() {
    let (mut doc, mut ids, ab) = base();
    let mut hist = History::new();
    let g = ids.mint();
    create(&mut hist, &mut doc, g, ab, NodeKind::Group);

    let stack = vec![Effect::new(EffectKind::LayerBlur { radius: 8.0 })];
    doc.apply(&Transaction(vec![Operation::SetEffects {
        id: g,
        effects: stack.clone(),
    }]))
    .expect("a group has no fill and still composites, so it takes an effect");
    assert_eq!(doc.get(g).unwrap().effects(), stack.as_slice());

    let err = doc
        .apply(&Transaction(vec![Operation::SetEffects {
            id: doc.root(),
            effects: stack,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WrongKindForOp));
}

/// `InsertSubtree` refuses a set holding a node nobody lists as a child.
///
/// ⚠️ **Composed from the public API alone — no `pub(crate)` access and no
/// hand-built `Node`.** Two `capture_subtree` snapshots of the same node taken at
/// different moments are enough: one of `P` before its child existed, one of the
/// child `X`. The set then holds `X` with `X.parent == Some(P)` while
/// `P.children` is empty, and `op_insert_subtree`'s structural pass walked only
/// *from each node to its listed children* — so `X` is not a root, is not
/// claimed, and is inserted anyway.
///
/// **Three panics and one unopenable file came out of that state**, all measured
/// by the review: `DeleteNode`, `Reparent` and `Reorder` on `X` each hit an
/// `expect("child must be listed in its parent")`, and `io::save` succeeded onto
/// a file whose `load` fails with *"node(s) are not reachable from the root"*.
/// The loader has enforced this since it was written — check (5) — so the two
/// doors into the tree were held to different standards, which §5.11 says they
/// must not be.
///
/// **No app gesture reaches it**, and that is why it is High rather than
/// Critical: both production `InsertSubtree` sites feed `remap_subtree` output
/// from a single fresh capture. What reaches it is anything that *composes*
/// captures — the MCP write surface, an importer, a multi-step builder — which is
/// precisely the traffic the model cannot vouch for. The module's own test doc
/// names that threat model and calls these subtrees "malformed subtrees the
/// public API can never produce"; the premise turns out to be false.
///
/// Flip-check, run: removing the `claimed.len() + 1 != nodes.len()` guard leaves
/// this failing at the first assertion — `apply` returns `Ok` — and the
/// `DeleteNode` assertion below then *panics* rather than failing, which is the
/// invariant-8 half and the more legible symptom of the two.
#[test]
fn a_subtree_holding_an_orphan_is_refused() {
    let mut ids = IdSource::new(0xA1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    let p = ids.mint();
    let x = ids.mint();

    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: p,
            parent: frame,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
    ]))
    .unwrap();

    // `P` as it is *now*: no children.
    let p_before = doc.capture_subtree(p).expect("P is in the document");
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: x,
        parent: p,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: Default::default(),
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    // And `X` as it is now: parented to `P`.
    let x_after = doc.capture_subtree(x).expect("X is in the document");

    let mut nodes = p_before;
    nodes.extend(x_after);
    assert_eq!(nodes.len(), 2, "the fixture is the two snapshots");

    let err = doc.apply(&Transaction(vec![
        Operation::DeleteNode { id: p },
        Operation::InsertSubtree {
            nodes,
            parent: frame,
            index: 0,
        },
    ]));
    // ⚠️ Matched rather than `assert!(matches!(…), "{err:?}")`: the `Ok` value
    // here is an `ApplyOutcome` carrying the whole inverse transaction, so the
    // formatted version prints two full `Node` dumps and buries the sentence.
    match err {
        Err(OpError::MalformedSubtree) => {}
        Err(e) => panic!("refused, but for the wrong reason: {e}"),
        Ok(_) => panic!("a set holding a node nobody claims as a child was accepted"),
    }

    // And nothing was left behind: `apply` is all-or-nothing, so `P` is still
    // where it was rather than deleted by the first operation of a failed
    // transaction.
    assert!(
        doc.get(p).is_some(),
        "the failed transaction changed nothing"
    );
    assert!(doc.get(x).is_some());

    // The invariant-8 half: whatever the document holds, an ordinary operation
    // on it must return rather than panic.
    assert!(
        doc.apply(&Transaction(vec![Operation::DeleteNode { id: x }]))
            .is_ok()
    );

    // The control: a well-formed subtree — one capture, all of it — still goes
    // in, or the guard is a feature removal.
    let whole = doc.capture_subtree(p).expect("P is still there");
    let n = whole.len();
    // Delete-then-reinsert, which is what undo does and the only way to put the
    // same ids back without colliding with themselves.
    doc.apply(&Transaction(vec![
        Operation::DeleteNode { id: p },
        Operation::InsertSubtree {
            nodes: whole,
            parent: frame,
            index: 0,
        },
    ]))
    .expect("a subtree captured in one piece is well-formed");
    assert_eq!(n, 1, "P alone by now — X was deleted above");
    assert!(doc.get(p).is_some());
}
