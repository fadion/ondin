use super::*;
use crate::{Guide, GuideAxis, GuideId, IdSource};

/// A frame with one guide down it and one across it, both scoped to the frame.
fn framed_guides() -> (Document, Resolved, NodeId, GuideId, GuideId) {
    let mut ids = IdSource::new(0x6D1DE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: frame,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 200.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    let (down, across) = (GuideId(ids.mint()), GuideId(ids.mint()));
    doc.apply(&Transaction(vec![
        Operation::AddGuide {
            guide: Guide {
                id: down,
                axis: GuideAxis::Vertical,
                position: 200.0,
                color: None,
                owner: frame,
            },
        },
        Operation::AddGuide {
            guide: Guide {
                id: across,
                axis: GuideAxis::Horizontal,
                position: 100.0,
                color: None,
                owner: frame,
            },
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, frame, down, across)
}

/// A resize holds a frame-scoped guide's offset; the Scale tool multiplies it by its own
/// axis's factor, not the geometric mean, which would drift a guide off its feature.
#[test]
fn the_scale_tool_multiplies_a_frames_guides_and_a_resize_does_not() {
    let corner = Point::new(800.0, 200.0);

    let (mut doc, res, frame, down, across) = framed_guides();
    doc.apply(&resize_to_handle(
        &doc,
        &res,
        frame,
        Handle::BottomRight,
        corner,
        Resize::default(),
    ))
    .unwrap();
    assert_eq!(
        doc.guide(down).unwrap().position,
        200.0,
        "a resize leaves an absolute length alone (§5.6)"
    );
    assert_eq!(doc.guide(across).unwrap().position, 100.0);

    // The Scale tool.
    let (mut doc, res, frame, down, across) = framed_guides();
    doc.apply(&resize_to_handle(
        &doc,
        &res,
        frame,
        Handle::BottomRight,
        corner,
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        doc.guide(down).unwrap().position,
        400.0,
        "the vertical guide takes sx = 2, so the guide down the middle of a \
         400-wide frame is down the middle of an 800-wide one"
    );
    assert_eq!(
        doc.guide(across).unwrap().position,
        100.0,
        "the horizontal guide takes sy = 1 — not the geometric mean, which \
         would have moved it to 141.4 under a scale that did not touch its axis"
    );
}

/// A photographic scale whose geometric mean is exactly 1 still moves the guides: the
/// `scaled_guides` call is not gated on `Scaling::scalar`, and each guide takes its own axis.
///
/// flip: put the `scaled_guides` call behind the `scalar` guard → fails at the first assertion
#[test]
fn a_photographic_scale_with_a_mean_of_one_still_moves_the_guides() {
    // 400x200 -> 800x100: sx = 2, sy = 0.5, and sqrt(|2 * 0.5|) == 1.
    let corner = Point::new(800.0, 100.0);
    let (mut doc, res, frame, down, across) = framed_guides();
    doc.apply(&resize_to_handle(
        &doc,
        &res,
        frame,
        Handle::BottomRight,
        corner,
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    ))
    .unwrap();

    assert_eq!(
        doc.guide(down).unwrap().position,
        400.0,
        "the vertical guide takes sx = 2 whatever the mean of the two factors is"
    );
    assert_eq!(
        doc.guide(across).unwrap().position,
        50.0,
        "and the horizontal one takes sy = 0.5 — each guide its own axis, \
         which is the half a single scalar cannot express"
    );
}

/// A guide on the frame's origin has nothing to scale, so scaling writes no operation for it.
#[test]
fn scaling_a_frame_writes_no_operation_for_a_guide_at_its_origin() {
    let mut ids = IdSource::new(0x6D1DE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: frame,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 200.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }]))
    .unwrap();
    let at_origin = GuideId(ids.mint());
    doc.apply(&Transaction(vec![Operation::AddGuide {
        guide: Guide {
            id: at_origin,
            axis: GuideAxis::Vertical,
            position: 0.0,
            color: None,
            owner: frame,
        },
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let tx = resize_to_handle(
        &doc,
        &res,
        frame,
        Handle::BottomRight,
        Point::new(800.0, 400.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    );
    assert!(
        !tx.0
            .iter()
            .any(|op| matches!(op, Operation::SetGuidePosition { .. })),
        "a guide on the origin needs no patch: {tx:?}"
    );
}

/// Scaling a frame leaves another frame's guides and global guides alone (the `owner` filter).
#[test]
fn scaling_a_frame_leaves_other_frames_and_global_guides_alone() {
    let mut ids = IdSource::new(0x6D1DE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (scaled, other) = (ids.mint(), ids.mint());
    let board = |id, index| Operation::CreateNode {
        id,
        parent: root,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 200.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    };
    doc.apply(&Transaction(vec![board(scaled, 0), board(other, 1)]))
        .unwrap();
    let (elsewhere, global) = (GuideId(ids.mint()), GuideId(ids.mint()));
    let guide = |id, owner| Operation::AddGuide {
        guide: Guide {
            id,
            axis: GuideAxis::Vertical,
            position: 120.0,
            color: None,
            owner,
        },
    };
    doc.apply(&Transaction(vec![
        guide(elsewhere, other),
        guide(global, root),
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    doc.apply(&resize_to_handle(
        &doc,
        &res,
        scaled,
        Handle::BottomRight,
        Point::new(800.0, 400.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(doc.guide(elsewhere).unwrap().position, 120.0);
    assert_eq!(doc.guide(global).unwrap().position, 120.0);
}

/// `scaled_layout` scales a grid's px lengths by their own axis (×2 across, ×0.5 down):
/// tracks, `minmax()` ends, `repeat()` tracks, gaps and padding; `fr` and `%` stay.
///
/// flip: the grid arm scales the columns by `csy` → fails at "the columns, across"
#[test]
fn a_grid_scales_its_tracks_gaps_and_padding_by_their_own_axis() {
    use crate::container::{Display, Grid, RepeatCount, Track, TrackBreadth as B, TrackSize as S};
    let (mut doc, _res, frame, ..) = framed_guides();
    let one = |b| Track::Size(S::Breadth(b));
    doc.apply(&Transaction(vec![Operation::SetDisplay {
        id: frame,
        display: Some(Display::Grid(Grid {
            columns: vec![
                one(B::Px(100.0)),
                one(B::Fr(1.0)),
                Track::Repeat {
                    repeat: RepeatCount::Count(2),
                    tracks: vec![S::MinMax {
                        min: B::Px(20.0),
                        max: B::Percent(10.0),
                    }],
                },
            ],
            rows: vec![one(B::Px(50.0)), one(B::Percent(25.0))],
            column_gap: 10.0,
            row_gap: 20.0,
            padding: [1.0, 2.0, 3.0, 4.0],
            ..Default::default()
        })),
    }]))
    .unwrap();
    let Some(Operation::SetDisplay {
        display: Some(Display::Grid(g)),
        ..
    }) = scaled_layout(&doc, frame, 2.0, 0.5)
    else {
        panic!("a grid's SetDisplay");
    };
    assert_eq!(
        g.columns,
        vec![
            one(B::Px(200.0)),
            one(B::Fr(1.0)),
            Track::Repeat {
                repeat: RepeatCount::Count(2),
                tracks: vec![S::MinMax {
                    min: B::Px(40.0),
                    max: B::Percent(10.0),
                }],
            },
        ],
        "the columns, across"
    );
    assert_eq!(
        g.rows,
        vec![one(B::Px(25.0)), one(B::Percent(25.0))],
        "the rows, down"
    );
    assert_eq!((g.column_gap, g.row_gap), (20.0, 10.0));
    assert_eq!(g.padding, [0.5, 4.0, 1.5, 8.0]);
}
