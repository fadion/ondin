//! Pages (`ondin_core::page`): the page operations and their inverses, the v10
//! save format and the migration onto it, a page's mode, the per-page spatial
//! index, and what a draft page keeps out of a kit.

use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Guide, GuideAxis, GuideId, IdSource, NodeId, NodeKind, OpError, Operation, Placement,
    Resolved, Transaction, hit_test, io,
};

fn frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(100.0, 100.0),
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(40.0, 40.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn create(id: NodeId, parent: NodeId, kind: NodeKind) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind,
        transform: None,
        name: None,
    }
}

fn add_page(id: NodeId, index: usize, name: &str) -> Operation {
    Operation::AddPage {
        id,
        index,
        name: name.into(),
        ground: Color::from_rgb8(10, 20, 30),
        draft: false,
        mode: None,
    }
}

/// A document of two pages, `Page 1` and `Pricing`, each holding one frame.
fn two_pages() -> (Document, IdSource, [NodeId; 4]) {
    let mut ids = IdSource::new(0x9A6E);
    let first = ids.mint();
    let mut doc = Document::new(first);
    let (second, a, b) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        add_page(second, 1, "Pricing"),
        create(a, first, frame()),
        create(b, second, frame()),
    ]))
    .unwrap();
    (doc, ids, [first, second, a, b])
}

/// Adding a page and removing it invert each other whole: the inverse of a
/// removal brings back the name, ground, draft flag, mode and place.
#[test]
fn a_page_comes_back_whole_from_its_removal() {
    let (mut doc, mut ids, [_, second, ..]) = two_pages();
    let third = ids.mint();
    doc.apply(&Transaction(vec![
        add_page(third, 1, "Archive"),
        Operation::SetPageDraft {
            id: third,
            draft: true,
        },
    ]))
    .unwrap();
    let before = doc.clone();
    let out = doc
        .apply(&Transaction(vec![Operation::RemovePage { id: third }]))
        .unwrap();
    assert!(!doc.is_page(third) && doc.get(third).is_none(), "removed");
    doc.apply(&out.inverse).unwrap();
    assert_eq!(doc, before, "the inverse restores the page as it was");
    assert_eq!(doc.page_index(third), Some(1));
    assert_eq!(doc.page_index(second), Some(2));
}

/// The last page, a page holding layers and a page holding a guide cannot be
/// removed: deleting a page is those deletes first.
#[test]
fn only_an_empty_page_that_is_not_the_last_is_removable() {
    let (mut doc, mut ids, [first, second, ..]) = two_pages();
    for page in [first, second] {
        let err = doc
            .apply(&Transaction(vec![Operation::RemovePage { id: page }]))
            .unwrap_err();
        assert!(
            matches!(err, OpError::PageNotRemovable(p) if p == page),
            "{err:?}"
        );
    }
    let empty = ids.mint();
    let guide = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Vertical,
        position: 5.0,
        color: None,
        owner: empty,
    };
    doc.apply(&Transaction(vec![
        add_page(empty, 2, "Empty"),
        Operation::AddGuide { guide },
    ]))
    .unwrap();
    assert!(
        doc.apply(&Transaction(vec![Operation::RemovePage { id: empty }]))
            .is_err(),
        "a page-wide guide keeps its page"
    );
    doc.apply(&Transaction(vec![
        Operation::RemoveGuide { id: guide.id },
        Operation::RemovePage { id: empty },
    ]))
    .unwrap();

    let mut one = Document::new(ids.mint());
    let only = one.first_page();
    assert!(
        one.apply(&Transaction(vec![Operation::RemovePage { id: only }]))
            .is_err(),
        "the last page stays"
    );
}

/// `MovePage` reorders and inverts to the old index; a page or the vault is not
/// a node any structural operation may move.
#[test]
fn pages_reorder_by_their_own_operation_only() {
    let (mut doc, _, [first, second, a, _]) = two_pages();
    let out = doc
        .apply(&Transaction(vec![Operation::MovePage {
            id: second,
            index: 0,
        }]))
        .unwrap();
    assert_eq!(doc.first_page(), second);
    doc.apply(&out.inverse).unwrap();
    assert_eq!(doc.first_page(), first);
    let err = doc
        .apply(&Transaction(vec![Operation::Reparent {
            id: second,
            new_parent: a,
            index: 0,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::CannotModifyRoot), "{err:?}");
}

/// A layer reparented to another page keeps its world position (both pages
/// share an origin), and `Resolved::update` agrees with a rebuild across the
/// move: the index is per page.
#[test]
fn a_layer_moved_between_pages_lands_in_that_pages_index() {
    let (mut doc, mut ids, [first, second, a, _]) = two_pages();
    let shape = ids.mint();
    let mut res = Resolved::rebuild(&doc);
    let out = doc
        .apply(&Transaction(vec![
            create(shape, a, rect()),
            Operation::SetTransform {
                id: shape,
                transform: Affine::translate((20.0, 20.0)),
            },
        ]))
        .unwrap();
    res.update(&doc, &out.dirty);
    let p = Point::new(30.0, 30.0);
    assert!(
        hit_test(&res, &doc, first, p, 0.0).contains(&shape),
        "fixture"
    );
    assert!(!hit_test(&res, &doc, second, p, 0.0).contains(&shape));

    let tx = ondin_core::reparent_preserving_world(&doc, &res, shape, second, 1).unwrap();
    let out = doc.apply(&tx).unwrap();
    res.update(&doc, &out.dirty);
    assert_eq!(doc.page_of(shape), Some(second));
    assert_eq!(
        res.world_bounds(shape),
        Resolved::rebuild(&doc).world_bounds(shape),
        "update agrees with rebuild"
    );
    assert!(
        !hit_test(&res, &doc, first, p, 0.0).contains(&shape),
        "gone from the first"
    );
    let fresh = Resolved::rebuild(&doc);
    assert!(
        hit_test(&fresh, &doc, second, p, 0.0).contains(&shape),
        "found on the second, at the same place"
    );
    assert!(
        hit_test(&res, &doc, second, p, 0.0).contains(&shape),
        "and by the live index"
    );
}

/// A page's mode is the last step a frame's mode inherits from before the
/// document's default; a frame's own still wins. A page may not set a kit's mode.
#[test]
fn a_frame_reads_its_pages_mode_unless_it_sets_one() {
    let (mut doc, mut ids, [first, _, a, _]) = two_pages();
    let (light, dark) = (
        ondin_core::variable::ModeId(ids.mint()),
        ondin_core::variable::ModeId(ids.mint()),
    );
    let mode = |id, name: &str, index| Operation::AddMode {
        mode: ondin_core::variable::Mode {
            id,
            name: name.into(),
        },
        index,
        cells: Vec::new(),
    };
    doc.apply(&Transaction(vec![
        mode(light, "Light", 0),
        mode(dark, "Dark", 1),
    ]))
    .unwrap();
    assert_eq!(doc.mode_at(a), Some(light), "the default first");
    doc.apply(&Transaction(vec![Operation::SetMode {
        id: first,
        mode: Some(dark),
    }]))
    .unwrap();
    assert_eq!(doc.mode_at(a), Some(dark), "the page's");
    doc.apply(&Transaction(vec![Operation::SetMode {
        id: a,
        mode: Some(light),
    }]))
    .unwrap();
    assert_eq!(doc.mode_at(a), Some(light), "the frame's own wins");
}

/// Several pages, a draft, a page mode and a page-wide guide save and load
/// byte-identically (invariant 9).
#[test]
fn pages_round_trip_through_a_file() {
    let (mut doc, mut ids, [_, second, ..]) = two_pages();
    doc.apply(&Transaction(vec![
        Operation::SetPageDraft {
            id: second,
            draft: true,
        },
        Operation::AddGuide {
            guide: Guide {
                id: GuideId(ids.mint()),
                axis: GuideAxis::Horizontal,
                position: 12.0,
                color: None,
                owner: second,
            },
        },
    ]))
    .unwrap();
    let bytes = io::save(&doc).unwrap();
    let back = io::load(&bytes).unwrap();
    assert_eq!(back, doc);
    assert_eq!(io::save(&back).unwrap(), bytes);
    assert!(back.page(second).unwrap().draft);
}

/// A v9 file — one `root`, a top-level `canvas_background`, a guide with no
/// owner, its root called `Root` — opens as one page carrying all three.
///
/// flip: `one_page` without the guide arm → fails at `load` (an owner is
/// required from v10).
#[test]
fn a_v9_file_opens_as_one_page() {
    let mut ids = IdSource::new(0x7E57);
    let root = ids.mint();
    let doc = Document::new(root);
    let mut v: serde_json::Value = serde_json::from_slice(&io::save(&doc).unwrap()).unwrap();
    let o = v.as_object_mut().unwrap();
    o.remove("pages");
    o.insert("schema_version".into(), 9.into());
    o.insert("root".into(), root.to_wire().into());
    o.insert(
        "canvas_background".into(),
        serde_json::to_value(Color::from_rgb8(1, 2, 3)).unwrap(),
    );
    o.insert(
        "guides".into(),
        serde_json::json!([{ "id": ids.mint().to_wire(), "axis": "vertical", "position": 4.0 }]),
    );
    v["nodes"][0]["name"] = "Root".into();

    let back = io::load(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(back.pages().len(), 1);
    assert_eq!(back.first_page(), root);
    assert_eq!(back.ground(root), Color::from_rgb8(1, 2, 3));
    assert_eq!(back.get(root).unwrap().name(), "Page 1");
    assert_eq!(
        back.guides()[0].owner,
        root,
        "the canvas-wide guide is the page's"
    );
}

/// Publishing leaves a draft page's mains out — unless a published main uses
/// one, which then goes with it and is named.
#[test]
fn a_draft_page_keeps_its_mains_out_of_a_kit_but_not_the_ones_used() {
    let (mut doc, mut ids, [_, second, a, b]) = two_pages();
    doc.apply(&Transaction(vec![
        Operation::SetComponent {
            id: a,
            component: true,
        },
        Operation::SetComponent {
            id: b,
            component: true,
        },
        Operation::SetPageDraft {
            id: second,
            draft: true,
        },
    ]))
    .unwrap();
    let tops = ondin_core::kit::freeze(&doc).unwrap();
    assert_eq!(tops.tops(), &[a], "the draft's main stays out");
    assert!(ondin_core::kit::carried_from_drafts(&doc).is_empty());

    // An instance of the draft's main inside the published one.
    let template = doc.capture_subtree(b).unwrap();
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: template,
            parent: a,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).unwrap();
    assert!(
        doc.get(made[0]).unwrap().link().is_some(),
        "fixture: an instance"
    );
    let tops = ondin_core::kit::freeze(&doc).unwrap();
    assert_eq!(tops.tops(), &[a, b], "the used draft main is carried");
    assert_eq!(ondin_core::kit::carried_from_drafts(&doc), vec![b]);
}
