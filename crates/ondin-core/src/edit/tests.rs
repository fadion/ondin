use super::*;
use crate::{IdSource, TextStyle};

/// A crop handle holds the opposite edges and stops at the picture.
#[test]
fn a_crop_handle_holds_the_opposite_edges_and_stops_at_the_picture() {
    let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
    let limit = Rect::new(-20.0, -10.0, 160.0, 140.0);

    let in_ = crop_resized(frame, Handle::BottomRight, Point::new(60.0, 70.0), limit);
    assert_eq!(in_, Rect::new(0.0, 0.0, 60.0, 70.0));

    let out = crop_resized(frame, Handle::BottomRight, Point::new(500.0, 500.0), limit);
    assert_eq!(out, Rect::new(0.0, 0.0, limit.x1, limit.y1));

    // An edge handle owns one axis only.
    let side = crop_resized(frame, Handle::Left, Point::new(-50.0, 999.0), limit);
    assert_eq!(
        side,
        Rect::new(limit.x0, 0.0, 100.0, 100.0),
        "a left drag moved an edge it does not own"
    );

    // A mirrored rectangle also has `x0 < x1`, hence the exact one.
    let past = crop_resized(
        frame,
        Handle::BottomRight,
        Point::new(-500.0, -500.0),
        limit,
    );
    assert_eq!(
        past,
        Rect::new(0.0, 0.0, MIN_CROP, MIN_CROP),
        "a handle run across the box must collapse onto the held corner, not \
         mirror about it"
    );
    let past_tl = crop_resized(frame, Handle::TopLeft, Point::new(500.0, 500.0), limit);
    assert_eq!(
        past_tl,
        Rect::new(100.0 - MIN_CROP, 100.0 - MIN_CROP, 100.0, 100.0),
        "the top-left handle must collapse onto the bottom-right corner"
    );
}

/// The cropped fill is the first visible image; hidden images and solids are skipped.
#[test]
fn the_cropped_fill_is_the_first_visible_image() {
    use crate::{Fill, Paint, image_brush};
    let img = |v: bool| Fill {
        brush: image_brush(crate::ImageId("a".into())).into(),
        visible: v,
    };
    let solid = Fill {
        brush: crate::Brush::Solid(crate::peniko::Color::BLACK).into(),
        visible: true,
    };
    let paint = |fills: Vec<Fill>| Paint {
        fills: crate::keyed_by_position(fills),
        strokes: Vec::new(),
    };
    assert_eq!(
        croppable_fill(&paint(vec![solid.clone(), img(true)])),
        Some(1)
    );
    assert_eq!(
        croppable_fill(&paint(vec![img(false), img(true)])),
        Some(1),
        "a hidden picture is not the one being cropped"
    );
    assert_eq!(croppable_fill(&paint(vec![solid.clone()])), None);
    assert_eq!(croppable_fill(&paint(vec![img(false)])), None);
    assert_eq!(croppable_fill(&paint(Vec::new())), None);
}

/// The box `crop_resized` computes is the box `resize_layer` writes, compared in
/// world space since a resize re-origins the local box.
#[test]
fn a_crop_resize_writes_the_very_box_its_crop_was_computed_against() {
    let (doc, res, rect) = artboard_with_rect();
    let frame = crate::local_box(&doc, &res, rect).expect("a box");
    let world = res.world_transform(rect).expect("a transform");
    let limit = frame.inflate(200.0, 200.0);

    for (handle, at) in [
        (Handle::BottomRight, Point::new(30.0, 20.0)),
        (Handle::TopLeft, Point::new(11.0, 7.0)),
        (Handle::Left, Point::new(-40.0, 0.0)),
        (Handle::Bottom, Point::new(0.0, 90.0)),
        // Past the held edge, where both floors must agree.
        (Handle::TopLeft, Point::new(999.0, 999.0)),
    ] {
        let next = crop_resized(frame, handle, at, limit);
        let (ux, uy) = handle.unit();
        let corner = Point::new(
            next.min_x() + next.width() * ux,
            next.min_y() + next.height() * uy,
        );
        let tx = resize_layer(
            &doc,
            &res,
            rect,
            handle,
            world * corner,
            Resize::geometry(false, false),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("the resize applies");
        let res_after = Resolved::rebuild(&after);
        let local = crate::local_box(&after, &res_after, rect).expect("a box after");
        let world_after = res_after.world_transform(rect).expect("a transform after");
        let want = world.transform_rect_bbox(next);
        let got = world_after.transform_rect_bbox(local);
        assert!(
            (got.x0 - want.x0).abs() < 1e-6
                && (got.y0 - want.y0).abs() < 1e-6
                && (got.x1 - want.x1).abs() < 1e-6
                && (got.y1 - want.y1).abs() < 1e-6,
            "{handle:?} to {at:?}: the crop was computed for {want:?} in world \
             space and the layer became {got:?} — the picture slides by the \
             difference"
        );
    }
}

/// A crop drag leaves the photograph where it was: the source-to-page transform
/// is identical before and after, and the crop stays inside the source.
#[test]
fn a_crop_drag_leaves_the_photograph_exactly_where_it_was() {
    let (doc, res, rect) = artboard_with_rect();
    let frame = crate::local_box(&doc, &res, rect).expect("a box");
    let world = res.world_transform(rect).expect("a transform");
    let (w, h) = (400u32, 300u32);
    let img = crate::ImageRef {
        fit: crate::ImageFit::Crop,
        crop: Rect::new(0.2, 0.1, 0.9, 0.6),
        ..crate::ImageRef::new(crate::ImageId("x".into()))
    };
    let limit = img.picture_box(frame, w, h).expect("a picture");
    let before = world * img.framing(frame, w, h).transform;

    for (handle, at) in [
        (Handle::BottomRight, Point::new(30.0, 20.0)),
        (Handle::TopLeft, Point::new(12.0, 9.0)),
        (Handle::Left, Point::new(-30.0, 0.0)),
        (Handle::Bottom, Point::new(0.0, 120.0)),
        // Out to the picture's own edge, where the clamp decides.
        (Handle::TopLeft, Point::new(-9_999.0, -9_999.0)),
    ] {
        let next = crop_resized(frame, handle, at, limit);
        let after_img = crate::ImageRef {
            crop: crate::crop_reframed(img.crop, frame, next),
            ..img.clone()
        };
        let (ux, uy) = handle.unit();
        let corner = Point::new(
            next.min_x() + next.width() * ux,
            next.min_y() + next.height() * uy,
        );
        let tx = resize_layer(
            &doc,
            &res,
            rect,
            handle,
            world * corner,
            Resize::geometry(false, false),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("the resize applies");
        let res_after = Resolved::rebuild(&after);
        let frame_after = crate::local_box(&after, &res_after, rect).expect("a box after");
        let world_after = res_after.world_transform(rect).expect("a transform after");
        let got = world_after * after_img.framing(frame_after, w, h).transform;

        let (a, b) = (before.as_coeffs(), got.as_coeffs());
        assert!(
            a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6),
            "{handle:?} to {at:?}: the picture was at {a:?} and is now at \
             {b:?} — cropping moved the photograph"
        );
        let c = after_img.crop;
        assert!(
            c.x0 >= -1e-9 && c.y0 >= -1e-9 && c.x1 <= 1.0 + 1e-9 && c.y1 <= 1.0 + 1e-9,
            "{handle:?} to {at:?}: the crop left the source: {c:?}"
        );
    }
}

fn artboard_with_rect() -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let rect = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: ab,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 50.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, rect)
}

/// A group holding one text node in `sizing`, offset inside it.
fn group_with_text(sizing: TextSizing) -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let group = ids.mint();
    let text = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: group,
            index: 0,
            kind: NodeKind::Text {
                content: "Hello there".into(),
                style: Box::new(crate::TextStyle {
                    font_family: "Inter".into(),
                    font_size: 16.0,
                    weight: 400,
                    italic: false,
                    line_height: Some(crate::Length::Em(1.2)),
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, group, text)
}

/// The text child's sizing after the group's bottom-right corner is dragged out
/// to double the box on both axes.
fn double_group(sizing: TextSizing) -> TextSizing {
    let (mut doc, res, group, text) = group_with_text(sizing);
    let local = crate::local_box(&doc, &res, group).unwrap();
    doc.apply(&resize_group(
        &doc,
        &res,
        group,
        local,
        Handle::BottomRight,
        Point::new(
            local.min_x() + 2.0 * local.width(),
            local.min_y() + 2.0 * local.height(),
        ),
        Resize::geometry(false, false),
    ))
    .unwrap();
    sizing_of(&doc, text)
}

/// Resizing a group leaves an auto-sized text child auto (not a 1×1 fixed box).
#[test]
fn scaling_a_group_leaves_auto_sized_text_alone() {
    let sizing = double_group(TextSizing::Auto);
    assert!(
        matches!(sizing, TextSizing::Auto),
        "auto text was converted to {sizing:?} by a group resize"
    );
}

/// A group resize scales an auto-height child's width and keeps it auto-height.
#[test]
fn scaling_a_group_scales_an_auto_height_child_without_fixing_it() {
    let sizing = double_group(TextSizing::AutoHeight(50.0));
    assert!(
        matches!(sizing, TextSizing::AutoHeight(w) if (w - 100.0).abs() < 1e-9),
        "expected auto-height at 100, got {sizing:?}"
    );
}

/// A group of two rects not starting at its own origin: local box 10,5..160,80.
fn group_of_two_rects() -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let group = ids.mint();
    let (a, b) = (ids.mint(), ids.mint());
    let rect = |size: Size| NodeKind::Rect {
        size,
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((30.0, 20.0))),
            name: None,
        },
        Operation::CreateNode {
            id: a,
            parent: group,
            index: 0,
            kind: rect(Size::new(40.0, 20.0)),
            transform: Some(Affine::translate((10.0, 5.0))),
            name: None,
        },
        Operation::CreateNode {
            id: b,
            parent: group,
            index: 1,
            kind: rect(Size::new(60.0, 30.0)),
            transform: Some(Affine::translate((100.0, 50.0))),
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, group)
}

/// Typing a group's W and H resizes it holding its top-left; the offset fixture
/// makes the position assertion catch scaling about (0,0).
/// flip: `resize_box_to` holds `Handle::TopLeft` → fails at the size assertion
#[test]
fn typing_a_width_on_a_group_resizes_it_from_its_own_top_left() {
    let (mut doc, res, group) = group_of_two_rects();
    let before = crate::local_box(&doc, &res, group).unwrap();
    assert!(
        (before.width() - 150.0).abs() < 1e-9 && (before.height() - 75.0).abs() < 1e-9,
        "the fixture's group measured {before:?} rather than 150 × 75"
    );
    doc.apply(&resize_box_to(&doc, &res, group, Size::new(300.0, 150.0)))
        .unwrap();
    let after = crate::local_box(&doc, &Resolved::rebuild(&doc), group).unwrap();
    assert!(
        (after.width() - 300.0).abs() < 1e-6 && (after.height() - 150.0).abs() < 1e-6,
        "asked for 300 × 150 and the group measured {after:?}"
    );
    assert!(
        (after.min_x() - before.min_x()).abs() < 1e-6
            && (after.min_y() - before.min_y()).abs() < 1e-6,
        "the group's top-left moved from {:?} to {:?}",
        before.origin(),
        after.origin()
    );
}

/// Dragging an auto-sized label's left edge widens it leftward and holds the
/// right edge, asserted there since both behaviours give the same new origin.
#[test]
fn dragging_an_auto_labels_left_edge_holds_its_right_edge() {
    let (mut doc, res, _group, text) = group_with_text(TextSizing::Auto);
    let measured = crate::local_box(&doc, &res, text).unwrap();
    let right = 10.0 + measured.width();
    let tx = resize_to_handle(
        &doc,
        &res,
        text,
        Handle::Left,
        Point::new(-30.0, 10.0),
        Resize::geometry(false, false),
    );
    doc.apply(&tx).unwrap();

    let sizing = sizing_of(&doc, text);
    let wanted = right + 30.0;
    assert!(
        matches!(sizing, TextSizing::AutoHeight(w) if (w - wanted).abs() < 0.5),
        "expected auto-height at {wanted} (the box from -30 to {right}), got {sizing:?}"
    );
    let res = Resolved::rebuild(&doc);
    let world = res.world_transform(text).unwrap();
    let origin = world * Point::ZERO;
    assert!(
        (origin.x - -30.0).abs() < 1e-9,
        "the dragged edge is not under the pointer: {origin:?}"
    );
    let far = world * Point::new(wanted, 0.0);
    assert!(
        (far.x - right).abs() < 0.5,
        "the held edge moved from {right} to {}",
        far.x
    );
}

/// A top drag on a trimmed auto label holds the bottom edge the handles drew
/// (the trimmed box starts below the local origin).
#[test]
fn dragging_the_top_of_a_trimmed_label_holds_the_bottom_the_handles_drew() {
    let (mut doc, _res, _group, text) = group_with_text(TextSizing::Auto);
    doc.apply(&Transaction(vec![Operation::SetBlockStyle {
        id: text,
        block: crate::BlockStyle {
            trim: crate::BoxTrim::CapToBaseline,
            ..Default::default()
        },
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let drawn = crate::local_box(&doc, &res, text).unwrap();
    assert!(
        drawn.min_y() > 0.1,
        "cap-to-baseline trim must inset the box from the origin: {drawn:?}"
    );
    let bottom = 10.0 + drawn.max_y();

    let tx = resize_to_handle(
        &doc,
        &res,
        text,
        Handle::Top,
        Point::new(10.0, -20.0),
        Resize::geometry(false, false),
    );
    doc.apply(&tx).unwrap();

    let TextSizing::Fixed(size) = sizing_of(&doc, text) else {
        panic!("a top drag authors the height");
    };
    let res = Resolved::rebuild(&doc);
    let after = crate::local_box(&doc, &res, text).unwrap();
    let world = res.world_transform(text).unwrap();
    let far = world * Point::new(0.0, after.max_y());
    assert!(
        (far.y - bottom).abs() < 0.5,
        "the held edge moved from {bottom} to {} (box {size:?}, drawn {after:?})",
        far.y
    );
    let top = world * Point::new(0.0, after.min_y());
    assert!(
        (top.y - -20.0).abs() < 0.5,
        "and the dragged edge must be under the pointer, not at {}",
        top.y
    );
}

#[test]
fn resize_from_bottom_right_only_changes_size() {
    let (mut doc, res, rect) = artboard_with_rect();
    let tx = resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::BottomRight,
        Point::new(260.0, 180.0),
        Resize::geometry(false, false),
    );
    doc.apply(&tx).unwrap();
    let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
        panic!();
    };
    assert!((size.width - 160.0).abs() < 1e-9);
    assert!((size.height - 80.0).abs() < 1e-9);
    let origin = Resolved::rebuild(&doc).world_transform(rect).unwrap() * Point::ZERO;
    assert!((origin.x - 100.0).abs() < 1e-9 && (origin.y - 100.0).abs() < 1e-9);
}

#[test]
fn resize_from_top_left_moves_the_origin_and_holds_the_far_corner() {
    let (mut doc, res, rect) = artboard_with_rect();
    let tx = resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::TopLeft,
        Point::new(120.0, 110.0),
        Resize::geometry(false, false),
    );
    doc.apply(&tx).unwrap();
    let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
        panic!();
    };
    assert!((size.width - 30.0).abs() < 1e-9, "w={}", size.width);
    assert!((size.height - 40.0).abs() < 1e-9, "h={}", size.height);

    let res = Resolved::rebuild(&doc);
    let w = res.world_transform(rect).unwrap();
    let origin = w * Point::ZERO;
    let far = w * Point::new(size.width, size.height);
    assert!((origin.x - 120.0).abs() < 1e-9 && (origin.y - 110.0).abs() < 1e-9);
    assert!(
        (far.x - 150.0).abs() < 1e-9 && (far.y - 150.0).abs() < 1e-9,
        "anchor corner moved: {far:?}"
    );
}

/// Every kind flips past its anchor, and the two resize paths agree: a `Rect` lands
/// in the same world box as a `Path`, so the assertion is on `Mirror`. Alt is the
/// same rule about the centre.
/// flip: `resized_box`'s `crossed` returns `false` → fails at the rect's `Mirror::X`
#[test]
fn a_rect_flips_past_its_anchor_exactly_as_a_path_does() {
    use crate::build::{Basis, Mirror};

    let rect = || NodeKind::Rect {
        size: Size::new(100.0, 100.0),
        corner_radii: crate::kurbo::RoundedRectRadii::new(16.0, 0.0, 0.0, 0.0),
        corner_smoothing: 0.0,
    };
    let path = || {
        let mut pen = BezPath::new();
        pen.move_to((0.0, 0.0));
        pen.line_to((100.0, 0.0));
        pen.line_to((100.0, 100.0));
        pen.line_to((0.0, 100.0));
        pen.close_path();
        NodeKind::Path {
            path: pen,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: crate::symmetry::PathMirror::NONE,
        }
    };

    // `(what, handle, pointer, symmetric)`: each drag runs against both kinds.
    let cases: [(&str, Handle, Point, bool); 3] = [
        (
            "the right handle dragged past the left edge",
            Handle::Right,
            Point::new(-50.0, 50.0),
            false,
        ),
        (
            "the right handle dragged past the centre with Alt",
            Handle::Right,
            Point::new(20.0, 50.0),
            true,
        ),
        // `Handle::Left` holds the right edge: only a drag past 100 crosses.
        (
            "the left handle dragged past the right edge",
            Handle::Left,
            Point::new(150.0, 50.0),
            false,
        ),
    ];

    for (what, handle, at, symmetric) in cases {
        let mut seen: Vec<(Mirror, Rect)> = Vec::new();
        for (label, kind) in [("rect", rect()), ("path", path())] {
            let mut ids = IdSource::new(0x1F3);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind,
                transform: None,
                name: None,
            }]))
            .expect("a node");
            let res = Resolved::rebuild(&doc);
            let tx = resize_layer(
                &doc,
                &res,
                id,
                handle,
                at,
                Resize::geometry(false, symmetric),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("applies");
            let ra = Resolved::rebuild(&after);
            let world = ra.world_transform(id).expect("a transform");
            let local = crate::local_box(&after, &ra, id).expect("a box");
            let ink: BezPath = crate::kurbo::Shape::to_path(&local, 0.01);
            let world_box = crate::kurbo::Shape::bounding_box(&(world * ink));
            let mirror = Basis::of(world).orientation.mirror;
            assert_eq!(
                mirror,
                Mirror::X,
                "{what}: the {label} is turned over — §5.6's flip, unqualified"
            );
            seen.push((mirror, world_box));
        }
        // Control: the two kinds land in the same box.
        let (_, rect_box) = seen[0];
        let (_, path_box) = seen[1];
        assert!(
            (rect_box.x0 - path_box.x0).abs() < 1e-6
                && (rect_box.x1 - path_box.x1).abs() < 1e-6
                && (rect_box.y0 - path_box.y0).abs() < 1e-6
                && (rect_box.y1 - path_box.y1).abs() < 1e-6,
            "{what}: the two kinds land in the same world box — \
             rect {rect_box:?} against path {path_box:?}"
        );
    }
}

/// A drag that stops short of the anchor flips nothing: the control for the test
/// above, which `crossed` returning `true` would pass.
/// flip: `crossed` spelled `at < anchor` → fails at "the left handle pulled in"
#[test]
fn an_ordinary_resize_flips_nothing() {
    use crate::build::{Basis, Mirror};

    for (what, handle, at) in [
        ("the right handle pulled out", Handle::Right, 150.0),
        (
            "the right handle pulled in, short of the edge",
            Handle::Right,
            30.0,
        ),
        ("the left handle pulled out", Handle::Left, -50.0),
        (
            "the left handle pulled in, short of the edge",
            Handle::Left,
            70.0,
        ),
    ] {
        let mut ids = IdSource::new(0x1F4);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        let res = Resolved::rebuild(&doc);
        let tx = resize_layer(
            &doc,
            &res,
            id,
            handle,
            Point::new(at, 50.0),
            Resize::geometry(false, false),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let ra = Resolved::rebuild(&after);
        let world = ra.world_transform(id).expect("a transform");
        assert_eq!(
            Basis::of(world).orientation.mirror,
            Mirror::None,
            "{what}: nothing was dragged past anything"
        );
    }
}

/// Dragging a handle past the opposite one flips a path: the bottom-right corner
/// of a 0..20 box dragged to -20 occupies -20..0 in world space, not a collapse.
#[test]
fn dragging_a_handle_past_the_opposite_corner_flips_a_path() {
    let mut ids = IdSource::new(0x1F1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let path = ids.mint();
    let mut pen = BezPath::new();
    pen.move_to((0.0, 0.0));
    pen.line_to((20.0, 0.0));
    pen.line_to((20.0, 20.0));
    pen.close_path();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: path,
        parent: root,
        index: 0,
        kind: NodeKind::Path {
            path: pen,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: crate::symmetry::PathMirror::NONE,
        },
        transform: None,
        name: None,
    }]))
    .expect("a path");
    let res = Resolved::rebuild(&doc);
    let local = crate::local_box(&doc, &res, path).expect("a box");

    let tx = resize_geometry(
        &doc,
        &res,
        path,
        local,
        Handle::BottomRight,
        Point::new(-20.0, -20.0),
        Resize::default(),
    );
    let mut after = doc.clone();
    after.apply(&tx).expect("applies");
    let res_after = Resolved::rebuild(&after);
    let world = res_after.world_transform(path).unwrap();
    let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
        panic!("still a path")
    };
    let now = crate::kurbo::Shape::bounding_box(&(world * shape.clone()));

    assert!(
        (now.x1 - 0.0).abs() < 1e-6 && (now.y1 - 0.0).abs() < 1e-6,
        "the held corner stayed at the origin: {now:?}"
    );
    assert!(
        (now.x0 + 20.0).abs() < 1e-6 && (now.y0 + 20.0).abs() < 1e-6,
        "and the shape is now on the other side of it, not collapsed: {now:?}"
    );
    assert!(
        now.width() > 19.0 && now.height() > 19.0,
        "it kept its size through the flip: {now:?}"
    );
}

/// The sign survives the clamp, and the magnitude is still clamped at both ends.
#[test]
fn a_scale_factor_keeps_its_sign_and_clamps_its_magnitude() {
    assert_eq!(scale_along(10.0, 20.0), 2.0);
    assert_eq!(
        scale_along(10.0, -20.0),
        -2.0,
        "a mirror is a negative factor"
    );
    assert_eq!(
        scale_along(10.0, 0.0),
        MIN_EXTENT,
        "and zero is still floored"
    );
    assert_eq!(
        scale_along(1.0, -1e9),
        -MAX_SCALE,
        "the cap applies to the magnitude, not to the value"
    );
    assert_eq!(scale_along(0.0, 5.0), 1.0, "no extent to have scaled");
}
/// Dragging a line's far end moves only that end.
#[test]
fn dragging_a_lines_end_moves_only_that_end() {
    let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)));
    let (start_before, _) = line_ends_world(&doc, &res, line).unwrap();

    let tx = move_line_end(&doc, &res, line, LineEnd::End, Point::new(200.0, 90.0));
    let mut after = doc.clone();
    after.apply(&tx).expect("applies");
    let res_after = Resolved::rebuild(&after);
    let (start, end) = line_ends_world(&after, &res_after, line).unwrap();

    approx_pt(
        end,
        Point::new(200.0, 90.0),
        "the dragged end went to the pointer",
    );
    approx_pt(start, start_before, "and the other end did not move");
}

/// Dragging a line's start holds the far end still: the start is the node origin
/// (§5.3), so `end` must be re-expressed.
#[test]
fn dragging_a_lines_start_holds_the_far_end_still() {
    let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)));
    let (_, end_before) = line_ends_world(&doc, &res, line).unwrap();

    let tx = move_line_end(&doc, &res, line, LineEnd::Start, Point::new(-30.0, 55.0));
    let mut after = doc.clone();
    after.apply(&tx).expect("applies");
    let res_after = Resolved::rebuild(&after);
    let (start, end) = line_ends_world(&after, &res_after, line).unwrap();

    approx_pt(
        start,
        Point::new(-30.0, 55.0),
        "the start went to the pointer",
    );
    approx_pt(
        end,
        end_before,
        "and the far end stayed exactly where it was",
    );
}

/// Moving either end of a rotated line keeps its basis.
#[test]
fn moving_an_end_of_a_rotated_line_keeps_its_basis() {
    // A quarter turn: the local x axis runs down the world y axis.
    let turn = Affine::rotate(std::f64::consts::FRAC_PI_2) * Affine::translate((0.0, 0.0));
    let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)) * turn);
    let basis_before = doc.get(line).unwrap().transform().as_coeffs();

    for (which, target) in [
        (LineEnd::End, Point::new(90.0, 120.0)),
        (LineEnd::Start, Point::new(-10.0, 5.0)),
    ] {
        let tx = move_line_end(&doc, &res, line, which, target);
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let (start, end) = line_ends_world(&after, &res_after, line).unwrap();
        let moved = match which {
            LineEnd::Start => start,
            LineEnd::End => end,
        };
        approx_pt(moved, target, &format!("{which:?} landed on the pointer"));
        let basis = after.get(line).unwrap().transform().as_coeffs();
        for i in 0..4 {
            assert!(
                (basis[i] - basis_before[i]).abs() < 1e-9,
                "{which:?}: the basis changed — coefficient {i} went {} → {}",
                basis_before[i],
                basis[i]
            );
        }
    }
}

/// Root → line at `at`, running 100 along its own x axis.
fn line_fixture(at: Affine) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x11D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let line = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: line,
        parent: root,
        index: 0,
        kind: NodeKind::Line {
            end: Point::new(100.0, 0.0),
        },
        transform: Some(at),
        name: None,
    }]))
    .expect("a line");
    let res = Resolved::rebuild(&doc);
    (doc, res, line)
}

fn approx_pt(got: Point, want: Point, what: &str) {
    assert!(
        (got.x - want.x).abs() < 1e-6 && (got.y - want.y).abs() < 1e-6,
        "{what}: {got:?} vs {want:?}"
    );
}
/// A lone line and a lone pen path scale from a handle. The path's box is away from
/// its origin, so the held edge stays put only with the `anchor·(1 − S)` correction.
#[test]
fn a_lone_line_and_path_scale_from_a_handle() {
    let mut ids = IdSource::new(0x11E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (line, path) = (ids.mint(), ids.mint());
    let mut pen = BezPath::new();
    pen.move_to((10.0, 0.0));
    pen.line_to((30.0, 0.0));
    pen.line_to((30.0, 40.0));
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: line,
            parent: root,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: path,
            parent: root,
            index: 1,
            kind: NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: crate::symmetry::PathMirror::NONE,
            },
            transform: None,
            name: None,
        },
    ]))
    .expect("a line and a path");
    let res = Resolved::rebuild(&doc);
    let opts = Resize::default();

    let local = crate::local_box(&doc, &res, line).expect("a line has a box");
    let tx = resize_geometry(
        &doc,
        &res,
        line,
        local,
        Handle::Right,
        Point::new(250.0, 0.0),
        opts,
    );
    assert!(!tx.0.is_empty(), "a line's handle drag must do something");
    let mut after = doc.clone();
    after.apply(&tx).expect("applies");
    let NodeKind::Line { end } = *after.get(line).unwrap().kind() else {
        panic!("still a line")
    };
    assert!(
        (end.x - 250.0).abs() < 1e-6 && end.y.abs() < 1e-6,
        "the endpoint follows the handle: {end:?}"
    );

    let local = crate::local_box(&doc, &res, path).expect("a path has a box");
    assert_eq!(
        (local.x0, local.x1),
        (10.0, 30.0),
        "the box is off the origin"
    );
    let tx = resize_geometry(
        &doc,
        &res,
        path,
        local,
        Handle::Right,
        Point::new(50.0, 20.0),
        opts,
    );
    assert!(!tx.0.is_empty(), "a path's handle drag must do something");
    let mut after = doc.clone();
    after.apply(&tx).expect("applies");
    // Measured in world space: only geometry and transform together hold the edge.
    let res_after = Resolved::rebuild(&after);
    let world = res_after.world_transform(path).unwrap();
    let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
        panic!("still a path")
    };
    let box_now = crate::kurbo::Shape::bounding_box(&(world * shape.clone()));
    assert!(
        (box_now.x0 - 10.0).abs() < 1e-6,
        "the held left edge stayed at 10: {box_now:?}"
    );
    assert!(
        (box_now.x1 - 50.0).abs() < 1e-6,
        "and the dragged right edge reached 50: {box_now:?}"
    );
    assert!(
        (box_now.y1 - 40.0).abs() < 1e-6,
        "a side drag left the other axis alone: {box_now:?}"
    );
}

/// Resizing a group scales its children's positions and geometry, not its own
/// transform (§5.6).
#[test]
fn resizing_a_group_scales_its_children_and_their_spacing() {
    let mut ids = IdSource::new(0x6E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, g, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: g,
            parent: ab,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a,
            parent: g,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: b,
            parent: g,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((80.0, 0.0))),
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let local = crate::local_box(&doc, &res, g).unwrap();
    assert_eq!((local.width(), local.height()), (100.0, 20.0));

    doc.apply(&resize_group(
        &doc,
        &res,
        g,
        local,
        Handle::BottomRight,
        Point::new(200.0, 20.0),
        Resize::geometry(false, false),
    ))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    for (id, want_x, want_w) in [(a, 0.0, 40.0), (b, 160.0, 40.0)] {
        let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
            panic!();
        };
        assert!(
            (size.width - want_w).abs() < 1e-9 && (size.height - 20.0).abs() < 1e-9,
            "child geometry {size:?}, want {want_w}x20"
        );
        let origin = res.world_transform(id).unwrap() * Point::ZERO;
        assert!(
            (origin.x - want_x).abs() < 1e-9,
            "child at {origin:?}, want x {want_x}"
        );
    }
    let after = crate::local_box(&doc, &res, g).unwrap();
    assert!((after.width() - 200.0).abs() < 1e-9, "{after:?}");
}

/// A group's own transform comes out untouched by a resize (§5.6).
#[test]
fn resizing_a_group_leaves_the_groups_own_transform_alone() {
    let mut ids = IdSource::new(0x77);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, g, a) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: g,
            parent: ab,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((30.0, 40.0))),
            name: None,
        },
        Operation::CreateNode {
            id: a,
            parent: g,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(50.0, 50.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let before = doc.get(g).unwrap().transform().as_coeffs();
    let local = crate::local_box(&doc, &res, g).unwrap();

    doc.apply(&resize_group(
        &doc,
        &res,
        g,
        local,
        Handle::BottomRight,
        Point::new(130.0, 140.0), // world → 100×100 in group space
        Resize::geometry(false, false),
    ))
    .unwrap();

    assert_eq!(doc.get(g).unwrap().transform().as_coeffs(), before);
    let NodeKind::Rect { size, .. } = doc.get(a).unwrap().kind() else {
        panic!();
    };
    assert!((size.width - 100.0).abs() < 1e-9, "{size:?}");
}

/// Dragging a side moves one axis and leaves the other, origin included, alone.
#[test]
fn a_side_handle_resizes_one_axis_and_leaves_the_other_untouched() {
    let size = Size::new(100.0, 60.0);
    for (handle, pointer, want_origin, want_size) in [
        (
            Handle::Right,
            Point::new(160.0, 12.0),
            Point::new(0.0, 0.0),
            Size::new(160.0, 60.0),
        ),
        (
            Handle::Left,
            Point::new(30.0, 12.0),
            Point::new(30.0, 0.0),
            Size::new(70.0, 60.0),
        ),
        (
            Handle::Bottom,
            Point::new(-40.0, 90.0),
            Point::new(0.0, 0.0),
            Size::new(100.0, 90.0),
        ),
        (
            Handle::Top,
            Point::new(-40.0, 20.0),
            Point::new(0.0, 20.0),
            Size::new(100.0, 40.0),
        ),
    ] {
        let (origin, got, _) = resized_box(size, handle, pointer, false, false);
        assert_eq!(got, want_size, "{handle:?} size");
        assert_eq!(origin, want_origin, "{handle:?} origin");
    }
}

/// A corner still drives both axes, and holds the opposite corner.
#[test]
fn a_corner_handle_still_drives_both_axes() {
    let size = Size::new(100.0, 60.0);
    let (origin, got, _) = resized_box(size, Handle::TopLeft, Point::new(20.0, 10.0), false, false);
    assert_eq!(got, Size::new(80.0, 50.0));
    assert_eq!(origin, Point::new(20.0, 10.0));
}

/// With Shift, a side drag carries the other axis with it.
#[test]
fn shift_on_a_side_scales_the_other_axis_too() {
    let (_, got, _) = resized_box(
        Size::new(100.0, 50.0),
        Handle::Right,
        Point::new(200.0, 0.0),
        true,
        false,
    );
    assert_eq!(got, Size::new(200.0, 100.0));
}

/// A moved pivot does not change what an Alt-resize does, asserted on the whole
/// transaction.
#[test]
fn a_moved_pivot_does_not_change_an_alt_resize() {
    use crate::Pivot;
    use crate::kurbo::Vec2;

    let alt_resize = |doc: &Document, res: &Resolved, id: NodeId| {
        resize_to_handle(
            doc,
            res,
            id,
            Handle::Right,
            Point::new(85.0, 30.0),
            Resize::geometry(false, true),
        )
    };

    let (doc, res, rect) = artboard_with_rect();
    let plain = alt_resize(&doc, &res, rect);

    for pivot in [
        Pivot::Normalized(Vec2::new(1.0, 1.0)),
        Pivot::Normalized(Vec2::new(0.98, 0.5)),
        Pivot::Normalized(Vec2::new(0.0, 0.0)),
        Pivot::Local(Point::new(3.0, 3.0)),
    ] {
        let (mut doc, res, rect) = artboard_with_rect();
        doc.apply(&Transaction(vec![Operation::SetPivot {
            id: rect,
            pivot: Some(pivot),
        }]))
        .expect("set pivot");
        assert_eq!(
            alt_resize(&doc, &res, rect),
            plain,
            "{pivot:?} changed the resize"
        );
    }
}

/// Alt holds the centre still: dragging one side moves the opposite one out to
/// match.
#[test]
fn alt_resizes_about_the_centre() {
    let size = Size::new(100.0, 50.0);
    let (origin, got, _) = resized_box(size, Handle::Right, Point::new(80.0, 0.0), false, true);
    assert_eq!(got, Size::new(60.0, 50.0));
    assert_eq!(origin, Point::new(20.0, 0.0));
    assert_eq!(origin.y, 0.0);

    // Without Alt the same drag holds the left edge.
    let (plain_origin, plain, _) =
        resized_box(size, Handle::Right, Point::new(80.0, 0.0), false, false);
    assert_eq!(plain, Size::new(80.0, 50.0));
    assert_eq!(plain_origin, Point::new(0.0, 0.0));

    let (o, s, _) = resized_box(
        size,
        Handle::BottomRight,
        Point::new(80.0, 40.0),
        false,
        true,
    );
    assert_eq!(s, Size::new(60.0, 30.0));
    assert_eq!(o, Point::new(20.0, 10.0));

    let (o, s, crossed) = resized_box(size, Handle::Right, Point::new(20.0, 0.0), false, true);
    assert_eq!(s, Size::new(60.0, 50.0));
    assert_eq!(o, Point::new(20.0, 0.0));
    assert_eq!(
        crossed,
        (true, false),
        "the pointer went past the centre on x and nowhere near it on y"
    );
}
/// Alt and Shift compose: proportional and centred.
#[test]
fn alt_and_shift_resize_proportionally_about_the_centre() {
    let size = Size::new(100.0, 50.0);
    let (origin, got, _) = resized_box(size, Handle::Right, Point::new(80.0, 0.0), true, true);
    assert_eq!(got, Size::new(60.0, 30.0));
    assert_eq!(origin, Point::new(20.0, 10.0));
}

/// Alt+Shift on a side handle centres the followed axis for a path too, as
/// `resized_box` does; the interesting half is the y pair.
/// flip: `resize_factors`' `pin` guards on `Some(_) if symmetric` → fails at the y assertion
#[test]
fn alt_and_shift_on_a_side_handle_centre_the_followed_axis_for_a_path_too() {
    let mut ids = IdSource::new(0x5B1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let path = ids.mint();
    let mut pen = BezPath::new();
    pen.move_to((0.0, 0.0));
    pen.line_to((100.0, 0.0));
    pen.line_to((100.0, 50.0));
    pen.line_to((0.0, 50.0));
    pen.close_path();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: path,
        parent: root,
        index: 0,
        kind: NodeKind::Path {
            path: pen,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: crate::symmetry::PathMirror::NONE,
        },
        transform: None,
        name: None,
    }]))
    .expect("a path");
    let res = Resolved::rebuild(&doc);

    let before = crate::local_box(&doc, &res, path).expect("a box");
    assert_eq!(before, Rect::new(0.0, 0.0, 100.0, 50.0));

    let tx = resize_layer(
        &doc,
        &res,
        path,
        Handle::Right,
        Point::new(80.0, 0.0),
        Resize::geometry(true, true),
    );
    let mut after = doc.clone();
    after.apply(&tx).expect("applies");
    let res_after = Resolved::rebuild(&after);
    let world = res_after.world_transform(path).unwrap();
    let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
        panic!("still a path")
    };
    let now = crate::kurbo::Shape::bounding_box(&(world * shape.clone()));

    assert!(
        (now.x0 - 20.0).abs() < 1e-9 && (now.x1 - 80.0).abs() < 1e-9,
        "the dragged axis is centred, as it always was: {now:?}"
    );
    assert!(
        (now.y0 - 10.0).abs() < 1e-9 && (now.y1 - 40.0).abs() < 1e-9,
        "and so is the axis the ratio lock drove: {now:?}"
    );
}

#[test]
fn resize_never_goes_below_one_unit() {
    let (mut doc, res, rect) = artboard_with_rect();
    let tx = resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::BottomRight,
        Point::new(100.0, 100.0),
        Resize::geometry(false, false),
    );
    doc.apply(&tx).unwrap();
    let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
        panic!();
    };
    assert!(size.width >= 1.0 && size.height >= 1.0);
}

#[test]
fn shift_resize_keeps_the_aspect_ratio() {
    let (mut doc, res, rect) = artboard_with_rect(); // 50x50, ratio 1
    let tx = resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::BottomRight,
        Point::new(300.0, 160.0),
        Resize::geometry(true, false),
    );
    doc.apply(&tx).unwrap();
    let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
        panic!();
    };
    assert!(
        (size.width - size.height).abs() < 1e-9,
        "expected a square, got {size:?}"
    );
}

/// A 400×400 artboard with one text node in `sizing` at its origin.
fn artboard_with_text(sizing: TextSizing) -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(9);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    let t = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: t,
            parent: ab,
            index: 0,
            kind: NodeKind::Text {
                content: "wrap me".into(),
                style: Box::new(crate::TextStyle {
                    font_family: "Inter".into(),
                    font_size: 16.0,
                    weight: 400,
                    italic: false,
                    line_height: Some(crate::Length::Em(1.2)),
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, t)
}

fn sizing_of(doc: &Document, id: NodeId) -> TextSizing {
    let NodeKind::Text { sizing, .. } = doc.get(id).unwrap().kind() else {
        panic!("not text");
    };
    *sizing
}

/// Dragging `handle` of the text node to `p`, and what its sizing became.
fn drag_text(sizing: TextSizing, handle: Handle, p: Point) -> TextSizing {
    let (mut doc, res, t) = artboard_with_text(sizing);
    let tx = resize_to_handle(&doc, &res, t, handle, p, Resize::geometry(false, false));
    doc.apply(&tx).unwrap();
    sizing_of(&doc, t)
}

#[test]
fn resizing_auto_text_from_a_corner_converts_it_to_a_fixed_box() {
    let sizing = drag_text(
        TextSizing::Auto,
        Handle::BottomRight,
        Point::new(80.0, 40.0),
    );
    let TextSizing::Fixed(size) = sizing else {
        panic!("expected a fixed box, got {sizing:?}");
    };
    assert!((size.width - 80.0).abs() < 1e-9 && (size.height - 40.0).abs() < 1e-9);
}

/// A side handle gives auto height, not a fixed box. The widths differ because the
/// right edge holds the origin while the left holds the far side (`anchored_box`).
#[test]
fn dragging_a_side_of_auto_text_gives_auto_height() {
    let (doc, res, t) = artboard_with_text(TextSizing::Auto);
    let ink = crate::local_box(&doc, &res, t).unwrap().width();
    for (handle, p, wanted) in [
        (Handle::Right, Point::new(80.0, 40.0), 80.0),
        (Handle::Left, Point::new(-30.0, 40.0), ink + 30.0),
    ] {
        let sizing = drag_text(TextSizing::Auto, handle, p);
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - wanted).abs() < 1e-9),
            "{handle:?} to {p:?} gave {sizing:?}, wanted auto-height at {wanted}"
        );
    }
}

/// The same drag on an already auto-height node keeps the mode (not a 1px-high box).
#[test]
fn dragging_a_side_of_auto_height_text_keeps_the_mode() {
    let sizing = drag_text(
        TextSizing::AutoHeight(50.0),
        Handle::Right,
        Point::new(120.0, 40.0),
    );
    assert!(
        matches!(sizing, TextSizing::AutoHeight(w) if (w - 120.0).abs() < 1e-9),
        "expected auto-height at 120, got {sizing:?}"
    );
}

/// The vertical handles fix an auto-height box, since the height becomes the user's.
#[test]
fn dragging_the_bottom_of_auto_height_text_fixes_the_box() {
    let sizing = drag_text(
        TextSizing::AutoHeight(50.0),
        Handle::Bottom,
        Point::new(0.0, 90.0),
    );
    assert!(
        matches!(sizing, TextSizing::Fixed(s) if (s.height - 90.0).abs() < 1e-9),
        "expected a 90-high fixed box, got {sizing:?}"
    );
}

/// Dragging a handle on railed text moves the ink: `TextSizing` is inert on a rail,
/// so the assertion is on the box. Dragging the bottom-right corner to twice the
/// width roughly doubles it and the held corner does not move (the rail arm writes
/// the translation itself, so the `SetTransform` shift must not apply it twice).
#[test]
fn dragging_railed_text_scales_its_rail() {
    use crate::kurbo::{BezPath, Shape};

    fn rail_box(doc: &Document, id: NodeId) -> Rect {
        let NodeKind::Text {
            on_path: Some(r), ..
        } = doc.get(id).unwrap().kind()
        else {
            panic!("not railed");
        };
        r.bounding_box()
    }

    let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
    let mut rail = BezPath::new();
    rail.move_to((0.0, 0.0));
    rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
    doc.apply(&Transaction(vec![Operation::SetGeometry {
        id: t,
        geometry: GeometryPatch::TextPath(Some(rail)),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let before = crate::local_box(&doc, &res, t).expect("a railed box");
    let rail_before = rail_box(&doc, t);
    assert!(
        before.width() > 1.0,
        "the fixture must have a box: {before:?}"
    );

    let target = Point::new(
        before.x0 + before.width() * 2.0,
        before.y0 + before.height(),
    );
    let tx = resize_to_handle(
        &doc,
        &res,
        t,
        Handle::BottomRight,
        target,
        Resize::geometry(false, false),
    );
    doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&doc);
    let after = crate::local_box(&doc, &res, t).expect("a railed box");
    let rail_after = rail_box(&doc, t);

    assert!(
        (after.width() - before.width() * 2.0).abs() < 0.5,
        "the box must land where the drag asked: {} -> {} (wanted {})",
        before.width(),
        after.width(),
        before.width() * 2.0
    );
    assert!(
        (after.x0 - before.x0).abs() < 0.01 && (after.y0 - before.y0).abs() < 0.01,
        "and the held corner must not move at all: {before:?} -> {after:?}"
    );
    // The drag owned `x` only; the residual is the solve's, not float noise.
    assert!(
        (rail_after.height() - rail_before.height()).abs() < 1e-3,
        "the rail must not move on the axis the drag held: {} -> {}",
        rail_before.height(),
        rail_after.height()
    );
    assert!(
        rail_after.width() > rail_before.width() * 1.5,
        "and it must actually have grown: {} -> {}",
        rail_before.width(),
        rail_after.width()
    );
}

/// Every handle holds its own corner on a rail, not just the `BottomRight` where
/// the origin is the anchor. Each handle shrinks the box to a quarter, since the
/// drift scales with distance from 1.
/// flip: `railed_resize` anchors `want.origin() - got.origin()` → fails at `BottomLeft`
#[test]
fn every_handle_holds_its_own_corner_on_a_rail() {
    use crate::kurbo::{BezPath, Vec2};

    for handle in [
        Handle::BottomRight,
        Handle::BottomLeft,
        Handle::TopRight,
        Handle::TopLeft,
    ] {
        let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
        let mut rail = BezPath::new();
        rail.move_to((0.0, 0.0));
        rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
        doc.apply(&Transaction(vec![Operation::SetGeometry {
            id: t,
            geometry: GeometryPatch::TextPath(Some(rail)),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = crate::local_box(&doc, &res, t).expect("a railed box");

        let (w, h) = (before.width() * 0.25, before.height() * 0.25);
        let (hx, hy) = handle.anchored(before.width(), before.height());
        // The corner this handle holds, and the pointer for a quarter-size box.
        let held = before.origin() + Vec2::new(hx.unwrap_or(0.0), hy.unwrap_or(0.0));
        let pointer = Point::new(
            match hx {
                Some(v) if v > 0.0 => held.x - w,
                _ => held.x + w,
            },
            match hy {
                Some(v) if v > 0.0 => held.y - h,
                _ => held.y + h,
            },
        );

        let tx = resize_to_handle(
            &doc,
            &res,
            t,
            handle,
            pointer,
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);
        let after = crate::local_box(&doc, &res, t).expect("a railed box");

        // The fixture must have shrunk, or "did not move" is vacuous.
        assert!(
            after.width() < before.width() * 0.5,
            "{handle:?}: the fixture must have shrunk: {} -> {}",
            before.width(),
            after.width()
        );

        let (ax, ay) = handle.anchored(after.width(), after.height());
        let held_after = after.origin() + Vec2::new(ax.unwrap_or(0.0), ay.unwrap_or(0.0));
        assert!(
            (held_after - held).hypot() < 0.05,
            "{handle:?}: the held corner slid {:.3} units — {held:?} -> {held_after:?}",
            (held_after - held).hypot()
        );
    }
}

/// The Scale tool on a rail holds the corner the Select tool holds: the solve must
/// run against the type the transaction leaves, since `scale_scalars` also
/// multiplies the font size. The anchor is asserted tightly, the extent loosely.
/// flip: `kind_to_solve_against` returns `None` unconditionally → fails at the anchor assertion
#[test]
fn the_scale_tool_on_a_rail_holds_the_corner_the_select_tool_holds() {
    use crate::kurbo::BezPath;

    let railed = || {
        let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
        let mut rail = BezPath::new();
        rail.move_to((0.0, 0.0));
        rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
        doc.apply(&Transaction(vec![Operation::SetGeometry {
            id: t,
            geometry: GeometryPatch::TextPath(Some(rail)),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, t)
    };

    let run = |scaling: Scaling| {
        let (mut doc, res, t) = railed();
        let before = crate::local_box(&doc, &res, t).expect("a railed box");
        let target = Point::new(
            before.x0 + before.width() * 2.0,
            before.y0 + before.height() * 2.0,
        );
        let tx = resize_to_handle(
            &doc,
            &res,
            t,
            Handle::BottomRight,
            target,
            Resize {
                scaling,
                ..Resize::geometry(false, false)
            },
        );
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);
        let after = crate::local_box(&doc, &res, t).expect("a railed box");
        (before, after)
    };

    let (before, select) = run(Scaling::Geometry);
    let (_, scale) = run(Scaling::Photographic);

    // The Select control lands the held corner exactly; Scale must match it.
    assert!(
        (select.x0 - before.x0).abs() < 0.01 && (select.y0 - before.y0).abs() < 0.01,
        "the Select control must hold the corner: {before:?} -> {select:?}"
    );
    assert!(
        (scale.x0 - before.x0).abs() < 0.01 && (scale.y0 - before.y0).abs() < 0.01,
        "the Scale tool must hold the same corner: {before:?} -> {scale:?}"
    );
    // The far corner is a target, so it is asserted loosely.
    let asked = before.height() * 2.0;
    assert!(
        (scale.height() - asked).abs() / asked < 0.05,
        "and the height must land near the drag: wanted {asked}, got {}",
        scale.height()
    );
}

/// Resizing a group that holds a railed text edits the rail too. The rect in the
/// group is the control that the gesture reached the subtree; the child's growth is
/// asserted loosely since the rail solve owns the exact width.
/// flip: delete the `on_path` arm of `scale_geometry` → fails at the railed text's width
#[test]
fn resizing_a_group_carries_a_railed_text_with_it() {
    use crate::kurbo::BezPath;

    let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
    let ab = doc.get(t).unwrap().parent().expect("the artboard");
    let mut ids = IdSource::new(0x9A11);
    let (group, rect) = (ids.mint(), ids.mint());
    let mut rail = BezPath::new();
    rail.move_to((0.0, 0.0));
    rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
    doc.apply(&Transaction(vec![
        Operation::SetGeometry {
            id: t,
            geometry: GeometryPatch::TextPath(Some(rail)),
        },
        Operation::CreateNode {
            id: group,
            parent: ab,
            index: 1,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: group,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(80.0, 40.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((0.0, 120.0))),
            name: None,
        },
        Operation::Reparent {
            id: t,
            new_parent: group,
            index: 0,
        },
    ]))
    .expect("group a railed text with a rect");

    let res = Resolved::rebuild(&doc);
    let g0 = crate::local_box(&doc, &res, group).expect("the group has a box");
    let t0 = crate::local_box(&doc, &res, t).expect("the text has a box");
    let r0 = crate::local_box(&doc, &res, rect).expect("the rect has a box");

    let world = res.world_transform(group).expect("world");
    let target = world * Point::new(g0.x0 + g0.width() * 2.0, g0.y0 + g0.height() * 2.0);
    let tx = resize_group(
        &doc,
        &res,
        group,
        g0,
        Handle::BottomRight,
        target,
        Resize::geometry(false, false),
    );
    doc.apply(&tx).expect("the group resize applies");
    let res = Resolved::rebuild(&doc);
    let t1 = crate::local_box(&doc, &res, t).expect("still a box");
    let r1 = crate::local_box(&doc, &res, rect).expect("still a box");

    assert!(
        (r1.width() - r0.width() * 2.0).abs() < 0.5,
        "control: the plain rect must double, {} -> {}",
        r0.width(),
        r1.width()
    );
    assert!(
        t1.width() > t0.width() * 1.5,
        "the railed text must grow with its sibling rather than stand still: \
         {} -> {} (unchanged is the defect)",
        t0.width(),
        t1.width()
    );
}

/// A fixed box is not un-fixed by a side drag: an authored height stays authored.
#[test]
fn dragging_a_side_of_fixed_text_keeps_it_fixed() {
    let sizing = drag_text(
        TextSizing::Fixed(Size::new(60.0, 30.0)),
        Handle::Right,
        Point::new(150.0, 0.0),
    );
    assert!(
        matches!(sizing, TextSizing::Fixed(s)
            if (s.width - 150.0).abs() < 1e-9 && (s.height - 30.0).abs() < 1e-9),
        "expected a 150×30 fixed box, got {sizing:?}"
    );
}

#[test]
fn create_rect_lands_at_world_drag_box_under_translated_parent() {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((50.0, 20.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let parent_world = res.world_transform(ab).unwrap();

    let rect = ids.mint();
    let tx = create_rect(
        rect,
        ab,
        0,
        parent_world,
        Point::new(100.0, 100.0),
        Point::new(160.0, 140.0),
        Color::from_rgba8(1, 2, 3, 255),
    );
    doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&doc);

    let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
        panic!("expected a rect");
    };
    assert!((size.width - 60.0).abs() < 1e-9);
    assert!((size.height - 40.0).abs() < 1e-9);
    let origin = res.world_transform(rect).unwrap() * Point::ZERO;
    assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
    assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
    assert!(doc.get(rect).unwrap().paint().fills.len() == 1);
}

/// A frame drawn inside a moved frame becomes its child and lands where it was
/// drawn (§5.3), which needs the parent's world transform.
#[test]
fn a_frame_drawn_inside_a_moved_frame_lands_where_it_was_drawn() {
    let mut ids = IdSource::new(0x5EED);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: page,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((50.0, 20.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let parent_world = res.world_transform(page).unwrap();

    let card = ids.mint();
    let tx = create_artboard(
        card,
        page,
        0,
        parent_world,
        Point::new(100.0, 100.0),
        Point::new(160.0, 140.0),
        Color::WHITE,
    );
    doc.apply(&tx).expect("a frame nests inside a frame");
    let res = Resolved::rebuild(&doc);

    assert_eq!(doc.get(card).unwrap().parent(), Some(page));
    let NodeKind::Artboard { size, .. } = doc.get(card).unwrap().kind() else {
        panic!("expected a frame");
    };
    assert!((size.width - 60.0).abs() < 1e-9);
    assert!((size.height - 40.0).abs() < 1e-9);
    let origin = res.world_transform(card).unwrap() * Point::ZERO;
    assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
    assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
}

/// A dragged text box is `Fixed` at what was drawn and lands where it was drawn
/// under a moved parent; the click plants `TextSizing::Auto` instead.
#[test]
fn a_dragged_text_box_is_fixed_at_the_drawn_size() {
    let mut ids = IdSource::new(0x7E87);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: page,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((50.0, 20.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let parent_world = res.world_transform(page).unwrap();

    let parts = crate::TextParts {
        style: crate::TextStyle {
            font_family: "Inter".into(),
            font_size: 16.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let t = ids.mint();
    let tx = create_text(
        t,
        page,
        0,
        parent_world,
        Point::new(100.0, 100.0),
        Point::new(260.0, 180.0),
        &parts,
    );
    doc.apply(&tx).expect("a text box nests inside a frame");
    let res = Resolved::rebuild(&doc);

    assert_eq!(doc.get(t).unwrap().parent(), Some(page));
    let NodeKind::Text {
        content, sizing, ..
    } = doc.get(t).unwrap().kind()
    else {
        panic!("expected a text node");
    };
    assert!(content.is_empty(), "created empty, to be typed into");
    let crate::TextSizing::Fixed(size) = sizing else {
        panic!("a drag asked for a width, so the mode is Fixed: {sizing:?}");
    };
    assert!((size.width - 160.0).abs() < 1e-9, "w={}", size.width);
    assert!((size.height - 80.0).abs() < 1e-9, "h={}", size.height);
    let origin = res.world_transform(t).unwrap() * Point::ZERO;
    assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
    assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
}

/// A thin drag still gets a box one line tall; the width has no floor.
#[test]
fn a_thin_text_drag_still_clears_one_line() {
    let mut ids = IdSource::new(0x7E88);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let parts = crate::TextParts {
        style: crate::TextStyle {
            font_family: "Inter".into(),
            font_size: 40.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let t = ids.mint();
    doc.apply(&create_text(
        t,
        root,
        0,
        Affine::IDENTITY,
        Point::new(10.0, 10.0),
        // Two units tall, well under one 40pt line.
        Point::new(38.0, 12.0),
        &parts,
    ))
    .unwrap();
    let NodeKind::Text { sizing, .. } = doc.get(t).unwrap().kind() else {
        panic!("expected a text node");
    };
    let crate::TextSizing::Fixed(size) = sizing else {
        panic!("{sizing:?}");
    };
    // "At least the type size", so this does not pin a metric.
    assert!(
        size.height >= 40.0,
        "a 2-unit drag should still hold a line of 40pt type, got {}",
        size.height
    );
    assert!(
        (size.width - 28.0).abs() < 1e-9,
        "and the width is left exactly as drawn: {}",
        size.width
    );
}

/// Text typed into a dragged box wraps to the drawn width, asserted on the shaped
/// layout; the box does not grow to fit, and `TextOverflow::Visible` lets text
/// hang out the bottom.
#[test]
fn a_dragged_text_box_wraps_what_is_typed_into_it() {
    let mut ids = IdSource::new(0x7E89);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let parts = crate::TextParts {
        style: crate::TextStyle {
            font_family: "Inter".into(),
            font_size: 16.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let t = ids.mint();
    doc.apply(&create_text(
        t,
        root,
        0,
        Affine::IDENTITY,
        Point::new(0.0, 0.0),
        Point::new(90.0, 200.0),
        &parts,
    ))
    .unwrap();

    let kind = doc.get(t).unwrap().kind();
    let node_parts = crate::TextParts::of(kind).expect("a text node");
    let sentence = "wrap this sentence over several lines";
    let mut edit = crate::TextEdit::new("", node_parts.clone());
    edit.insert(sentence);
    let laid = edit.text_layout();

    let mut auto = node_parts;
    auto.sizing = crate::TextSizing::Auto;
    let unwrapped = crate::text::layout(auto.as_ref(sentence));
    assert!(
        unwrapped.size.width > 150.0,
        "the fixture has to be wider than the box to prove anything: {}",
        unwrapped.size.width
    );
    assert!(
        (laid.size.width - 90.0).abs() < 1e-9,
        "the box keeps the drawn width: {}",
        laid.size.width
    );
    assert!(
        (laid.size.height - 200.0).abs() < 1e-9,
        "and the drawn height, rather than growing to the text: {}",
        laid.size.height
    );
    // Distinct baselines is the wrap, without pinning how many lines Inter takes.
    let mut baselines: Vec<i32> = laid
        .runs
        .iter()
        .filter_map(|r| r.glyphs.first().map(|g| (g.y * 100.0) as i32))
        .collect();
    baselines.sort_unstable();
    baselines.dedup();
    assert!(
        baselines.len() > 1,
        "a 90-unit box must wrap this sentence: {baselines:?}"
    );
}

#[test]
fn create_shape_box_is_order_independent() {
    let pw = Affine::IDENTITY;
    let (o1, s1) = local_box(pw, Point::new(10.0, 10.0), Point::new(50.0, 30.0));
    let (o2, s2) = local_box(pw, Point::new(50.0, 30.0), Point::new(10.0, 10.0));
    assert_eq!(o1, o2);
    assert_eq!(s1, s2);
    assert_eq!(o1, Point::new(10.0, 10.0));
    assert_eq!(s1, Size::new(40.0, 20.0));
}

#[test]
fn create_path_points_map_back_to_world() {
    use crate::kurbo::PathEl;
    let mut ids = IdSource::new(4);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((12.0, 34.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let pw = res.world_transform(ab).unwrap();

    let path = ids.mint();
    let world_pts = [
        Point::new(50.0, 60.0),
        Point::new(120.0, 60.0),
        Point::new(120.0, 140.0),
    ];
    let anchors: Vec<PenAnchor> = world_pts.iter().copied().map(PenAnchor::corner).collect();
    doc.apply(&create_path(
        path,
        ab,
        0,
        pw,
        &anchors,
        false,
        solid_stroke(Color::BLACK, 2.0),
    ))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let w = res.world_transform(path).unwrap();
    let NodeKind::Path { path: bez, .. } = doc.get(path).unwrap().kind() else {
        panic!("expected a path");
    };
    let local_pts: Vec<Point> = bez
        .elements()
        .iter()
        .filter_map(|el| match el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
            _ => None,
        })
        .collect();
    assert_eq!(local_pts.len(), 3);
    for (local, expected) in local_pts.iter().zip(world_pts.iter()) {
        let got = w * *local;
        assert!(
            (got.x - expected.x).abs() < 1e-9 && (got.y - expected.y).abs() < 1e-9,
            "{got:?} vs {expected:?}"
        );
    }
}

/// The close snap radius is screen pt, so the world tolerance shrinks as the view
/// zooms in; comparing world distance directly would fail only the zoom case.
#[test]
fn the_pen_snaps_closed_within_a_screen_radius_not_a_world_one() {
    const R: f64 = 5.0;
    let square = [
        PenAnchor::corner(Point::new(0.0, 0.0)),
        PenAnchor::corner(Point::new(100.0, 0.0)),
        PenAnchor::corner(Point::new(100.0, 100.0)),
    ];

    assert_eq!(
        pen_close_snap(&square, Point::new(4.0, 0.0), 1.0, R),
        Some(Point::new(0.0, 0.0))
    );
    assert_eq!(pen_close_snap(&square, Point::new(40.0, 0.0), 1.0, R), None);

    // At 4x the same 4 units is 16 screen pt away.
    assert_eq!(
        pen_close_snap(&square, Point::new(4.0, 0.0), 4.0, R),
        None,
        "the radius is screen pt: zooming in must tighten the world tolerance"
    );
    assert_eq!(
        pen_close_snap(&square, Point::new(4.0, 0.0), 0.5, R),
        Some(Point::new(0.0, 0.0))
    );

    // `create_path` refuses a one-anchor path, so it must not offer a close.
    let lone = [PenAnchor::corner(Point::new(0.0, 0.0))];
    assert_eq!(pen_close_snap(&lone, Point::new(0.0, 0.0), 1.0, R), None);
    assert_eq!(pen_close_snap(&[], Point::new(0.0, 0.0), 1.0, R), None);

    assert_eq!(
        pen_close_snap(&square, Point::new(100.0, 0.0), 1.0, R),
        None,
        "the second anchor is not a close target"
    );
}

/// A pen anchor with handles produces a curve and one without produces a line,
/// so a clicked-out path stays a polyline.
#[test]
fn pen_anchors_curve_only_where_they_have_handles() {
    use crate::kurbo::{PathEl, Vec2};
    let corner = |x, y| PenAnchor::corner(Point::new(x, y));
    let smooth = |x, y, hx, hy| PenAnchor::smooth(Point::new(x, y), Vec2::new(hx, hy));

    let straight = pen_outline(&[corner(0.0, 0.0), corner(10.0, 0.0)], false);
    assert!(
        matches!(straight.elements()[1], PathEl::LineTo(_)),
        "two corners are a line: {:?}",
        straight.elements()
    );

    let curved = pen_outline(&[smooth(0.0, 0.0, 5.0, 5.0), corner(10.0, 0.0)], false);
    let PathEl::CurveTo(c1, c2, end) = curved.elements()[1] else {
        panic!("a handled anchor should curve: {:?}", curved.elements());
    };
    assert_eq!(c1, Point::new(5.0, 5.0), "leaves along the handle");
    assert_eq!(c2, Point::new(10.0, 0.0), "a corner has no incoming handle");
    assert_eq!(end, Point::new(10.0, 0.0));

    let closed = pen_outline(
        &[corner(0.0, 0.0), corner(10.0, 0.0), corner(10.0, 10.0)],
        true,
    );
    assert!(
        matches!(closed.elements().last(), Some(PathEl::ClosePath)),
        "{:?}",
        closed.elements()
    );
}

/// A broken handle pair arrives straight and leaves on a curve, which a mirrored
/// anchor cannot say.
#[test]
fn an_anchor_can_break_its_handles_and_bend_only_one_side() {
    use crate::kurbo::{PathEl, Vec2};

    let broken = PenAnchor {
        at: Point::new(10.0, 0.0),
        out: Vec2::new(5.0, 5.0),
        back: Vec2::ZERO,
        radius: 0.0,
    };
    let path = pen_outline(
        &[
            PenAnchor::corner(Point::new(0.0, 0.0)),
            broken,
            PenAnchor::corner(Point::new(20.0, 0.0)),
        ],
        false,
    );

    // Into the broken anchor there is no incoming handle, so the segment is straight.
    assert_eq!(
        path.elements()[1],
        PathEl::LineTo(Point::new(10.0, 0.0)),
        "the incoming side was broken, so the segment into it must not bend: {:?}",
        path.elements()
    );
    let PathEl::CurveTo(c1, _, _) = path.elements()[2] else {
        panic!(
            "out of a handled anchor should curve: {:?}",
            path.elements()
        );
    };
    assert_eq!(c1, Point::new(15.0, 5.0), "leaves along `out`");

    // Mirrored, the same anchor bends both ways.
    let mirrored = pen_outline(
        &[
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::smooth(Point::new(10.0, 0.0), Vec2::new(5.0, 5.0)),
            PenAnchor::corner(Point::new(20.0, 0.0)),
        ],
        false,
    );
    let PathEl::CurveTo(_, c2m, _) = mirrored.elements()[1] else {
        panic!("{:?}", mirrored.elements());
    };
    assert_eq!(
        c2m,
        Point::new(5.0, -5.0),
        "mirrored, the incoming control point is the reflection of `out`"
    );
}

/// `pull_within` mirrors the handle pair, breaks it under Alt, re-mirrors when Alt is
/// released mid-drag, and retracts a handle inside the radius so a snapped click stays a corner.
#[test]
fn pulling_a_handle_mirrors_unless_the_pair_is_broken() {
    use crate::kurbo::Vec2;
    let at = Point::new(10.0, 10.0);
    let pull = |a: &mut PenAnchor, to: Point, alt: bool| a.pull_within(Side::Out, to, alt, 0.0);

    let mut a = PenAnchor::corner(at);
    pull(&mut a, Point::new(15.0, 10.0), false);
    assert_eq!(a.out, Vec2::new(5.0, 0.0));
    assert_eq!(a.back, Vec2::new(-5.0, 0.0), "mirrored");

    // Alt: the incoming handle stays exactly where the mirror left it.
    pull(&mut a, Point::new(10.0, 20.0), true);
    assert_eq!(a.out, Vec2::new(0.0, 10.0));
    assert_eq!(
        a.back,
        Vec2::new(-5.0, 0.0),
        "a broken pair leaves the incoming handle alone"
    );

    // Releasing Alt re-mirrors, rather than leaving the anchor broken for good.
    pull(&mut a, Point::new(10.0, 20.0), false);
    assert_eq!(a.back, Vec2::new(0.0, -10.0));

    a.pull_within(Side::Out, Point::new(12.0, 11.0), false, 7.0);
    assert!(a.is_corner(), "out={:?} back={:?}", a.out, a.back);
    a.pull_within(Side::Out, Point::new(30.0, 10.0), false, 7.0);
    assert_eq!(a.out, Vec2::new(20.0, 0.0));
}
/// A corner needs both handles to be zero; an incoming-only anchor would straighten a curve.
#[test]
fn an_anchor_with_only_an_incoming_handle_is_not_a_corner() {
    use crate::kurbo::Vec2;
    let incoming_only = PenAnchor {
        at: Point::new(1.0, 1.0),
        out: Vec2::ZERO,
        back: Vec2::new(-3.0, 0.0),
        radius: 0.0,
    };
    assert!(!incoming_only.is_corner());
    assert!(PenAnchor::corner(Point::new(1.0, 1.0)).is_corner());
}

/// `create_path` keeps a curve's control points in step with its anchors through the
/// local-space conversion.
#[test]
fn a_curved_pen_path_keeps_its_control_points_in_world_space() {
    use crate::kurbo::{PathEl, Vec2};
    let mut ids = IdSource::new(11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((17.0, 23.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let pw = res.world_transform(ab).unwrap();

    let anchors = [
        PenAnchor::smooth(Point::new(50.0, 60.0), Vec2::new(20.0, -20.0)),
        PenAnchor::corner(Point::new(150.0, 60.0)),
    ];
    let want = pen_outline(&anchors, false);

    let id = ids.mint();
    doc.apply(&create_path(
        id,
        ab,
        0,
        pw,
        &anchors,
        false,
        solid_stroke(Color::BLACK, 2.0),
    ))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let world = res.world_transform(id).unwrap();
    let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
        panic!("expected a path");
    };

    let mut back = path.clone();
    back.apply_affine(world);
    let (a, b) = (want.elements(), back.elements());
    assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
    for (x, y) in a.iter().zip(b.iter()) {
        let pts = |el: &PathEl| match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
            PathEl::QuadTo(c, p) => vec![c, p],
            PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
            PathEl::ClosePath => vec![],
        };
        let (px, py) = (pts(x), pts(y));
        assert_eq!(px.len(), py.len(), "{x:?} vs {y:?}");
        for (p, q) in px.iter().zip(py.iter()) {
            assert!(
                (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9,
                "{p:?} vs {q:?}"
            );
        }
    }
}

/// Points of a path element, for comparing two paths element by element.
fn el_points(el: &PathEl) -> Vec<Point> {
    match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
        PathEl::QuadTo(c, p) => vec![c, p],
        PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
        PathEl::ClosePath => vec![],
    }
}

/// Two paths describe the same curve, element for element.
fn assert_same_path(a: &BezPath, b: &BezPath) {
    let (ea, eb) = (a.elements(), b.elements());
    assert_eq!(
        ea.len(),
        eb.len(),
        "different element counts:\n{ea:#?}\nvs\n{eb:#?}"
    );
    for (x, y) in ea.iter().zip(eb.iter()) {
        assert_eq!(
            std::mem::discriminant(x),
            std::mem::discriminant(y),
            "{x:?} vs {y:?}"
        );
        for (p, q) in el_points(x).iter().zip(el_points(y).iter()) {
            assert!(
                (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9,
                "{x:?} vs {y:?}"
            );
        }
    }
}

/// `pen_anchors` inverts `pen_outline`; a closed path's repeated final point must not
/// read as an extra anchor, hence the anchor count.
#[test]
fn reading_a_pen_path_back_gives_the_anchors_that_drew_it() {
    use crate::kurbo::Vec2;

    let anchors = vec![
        PenAnchor::corner(Point::new(0.0, 0.0)),
        PenAnchor::smooth(Point::new(40.0, 30.0), Vec2::new(10.0, 0.0)),
        // A broken pair: arrives straight, leaves on a curve.
        PenAnchor {
            at: Point::new(80.0, 0.0),
            out: Vec2::new(0.0, -12.0),
            back: Vec2::ZERO,
            radius: 0.0,
        },
        PenAnchor::corner(Point::new(120.0, 20.0)),
    ];

    for closed in [false, true] {
        let drawn = pen_outline(&anchors, closed);
        let read = pen_anchors(&drawn, &[]);
        assert_eq!(read.len(), 1, "one `MoveTo` is one subpath");
        assert_eq!(read[0].closed, closed);
        assert_eq!(
            read[0].anchors.len(),
            anchors.len(),
            "closed={closed}: the closing segment is not an anchor of its own"
        );
        for (want, got) in anchors.iter().zip(read[0].anchors.iter()) {
            assert_eq!((want.at, want.out, want.back), (got.at, got.out, got.back));
        }
        assert_same_path(&drawn, &pen_path(&read));
    }
}

/// The reader keeps every subpath of a general path and elevates quads to the same
/// curve as cubics, the way kurbo does.
#[test]
fn reading_a_general_path_elevates_quads_and_keeps_every_subpath() {
    use crate::kurbo::QuadBez;

    let mut path = BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((10.0, 0.0));
    path.quad_to((20.0, 10.0), (30.0, 0.0));
    path.move_to((100.0, 0.0));
    path.line_to((140.0, 0.0));
    path.line_to((140.0, 40.0));
    path.close_path();

    let read = pen_anchors(&path, &[]);
    assert_eq!(read.len(), 2, "two `MoveTo`s are two subpaths");
    assert!(!read[0].closed);
    assert_eq!(read[0].anchors.len(), 3);
    assert!(read[1].closed);
    assert_eq!(
        read[1].anchors.len(),
        3,
        "an implied closing edge adds no anchor"
    );

    let raised = QuadBez::new((10.0, 0.0), (20.0, 10.0), (30.0, 0.0)).raise();
    let (a, b) = (&read[0].anchors[1], &read[0].anchors[2]);
    assert_eq!(a.leaving(), raised.p1);
    assert_eq!(b.arriving(), raised.p2);
    assert!(a.back.hypot() < 1e-9, "a line arrived at it");

    let out = pen_path(&read);
    let mut want = BezPath::new();
    want.move_to((0.0, 0.0));
    want.line_to((10.0, 0.0));
    want.curve_to(raised.p1, raised.p2, raised.p3);
    want.move_to((100.0, 0.0));
    want.line_to((140.0, 0.0));
    want.line_to((140.0, 40.0));
    want.line_to((100.0, 0.0));
    want.close_path();
    assert_same_path(&out, &want);
}

/// Reversing a subpath swaps its handles so the same curve is drawn backwards.
#[test]
fn reversing_a_subpath_draws_the_same_curve_backwards() {
    use crate::kurbo::Vec2;

    let mut sub = PenSubpath {
        anchors: vec![
            PenAnchor::smooth(Point::new(0.0, 0.0), Vec2::new(10.0, 10.0)),
            PenAnchor::smooth(Point::new(60.0, 0.0), Vec2::new(10.0, -10.0)),
        ],
        closed: false,
    };
    let forward = pen_path(std::slice::from_ref(&sub));
    sub.reverse();
    let back = pen_path(std::slice::from_ref(&sub));

    let mut want = BezPath::new();
    want.move_to((60.0, 0.0));
    want.curve_to((50.0, 10.0), (10.0, 10.0), (0.0, 0.0));
    assert_same_path(&back, &want);

    // Reversal is an involution.
    sub.reverse();
    assert_same_path(&pen_path(std::slice::from_ref(&sub)), &forward);
}

/// `rewrite_path` goes back through the node's own world transform, so extending a
/// path's start leaves its transform and its other subpaths alone.
#[test]
fn rewriting_a_path_leaves_its_transform_and_its_other_subpaths_alone() {
    let mut ids = IdSource::new(21);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((17.0, 23.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let pw = res.world_transform(ab).unwrap();

    let drawn = [
        PenAnchor::corner(Point::new(50.0, 60.0)),
        PenAnchor::corner(Point::new(150.0, 60.0)),
    ];
    let id = ids.mint();
    doc.apply(&create_path(
        id,
        ab,
        0,
        pw,
        &drawn,
        false,
        solid_stroke(Color::BLACK, 2.0),
    ))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let before = doc.get(id).unwrap().transform();
    let node_world = res.world_transform(id).unwrap();

    let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
        panic!("expected a path");
    };
    let current = path.clone();
    let mut subpaths = pen_anchors(&current, &[]);
    map_subpaths(&mut subpaths, node_world);
    subpaths.push(PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(200.0, 200.0)),
            PenAnchor::corner(Point::new(260.0, 200.0)),
        ],
        closed: true,
    });

    // Resuming and changing nothing is not an edit, even after a reverse round trip.
    let mut untouched = subpaths[0].clone();
    untouched.reverse();
    untouched.reverse();
    let idle = PenSubpath {
        anchors: untouched.anchors,
        closed: false,
    };
    let mut idle_all = subpaths.clone();
    idle_all[0] = idle;
    assert!(
        rewrite_path(id, node_world, &idle_all[..1], Some((&current, &[])))
            .0
            .is_empty(),
        "a resume that changed nothing must not become an undo step"
    );

    let mut working = subpaths[0].clone();
    working.reverse();
    working
        .anchors
        .push(PenAnchor::corner(Point::new(50.0, 140.0)));
    working.reverse();
    subpaths[0] = working;

    doc.apply(&rewrite_path(
        id,
        node_world,
        &subpaths,
        Some((&current, &[])),
    ))
    .unwrap();
    let res = Resolved::rebuild(&doc);

    assert_eq!(
        doc.get(id).unwrap().transform(),
        before,
        "extending the start of a path must not move the node"
    );
    let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
        panic!("expected a path");
    };
    let mut world = path.clone();
    world.apply_affine(res.world_transform(id).unwrap());
    let read = pen_anchors(&world, &[]);
    assert_eq!(read.len(), 2, "the untouched subpath is still there");
    let ends: Vec<Point> = read[0].anchors.iter().map(|a| a.at).collect();
    assert_eq!(
        ends,
        vec![
            Point::new(50.0, 140.0),
            Point::new(50.0, 60.0),
            Point::new(150.0, 60.0)
        ],
        "extending the start prepends, leaving the path's direction alone"
    );
    assert!(read[1].closed);
    assert_eq!(read[1].anchors[0].at, Point::new(200.0, 200.0));
}

/// Moving a point carries its handle offsets with it, so the curve leaves it the same way.
#[test]
fn moving_a_point_carries_its_handles_and_leaves_its_neighbours_shaped() {
    use crate::kurbo::Vec2;
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::smooth(Point::new(0.0, 0.0), Vec2::new(10.0, 0.0)),
            PenAnchor::smooth(Point::new(50.0, 0.0), Vec2::new(10.0, 10.0)),
            PenAnchor::smooth(Point::new(100.0, 0.0), Vec2::new(10.0, 0.0)),
        ],
        closed: false,
    }];
    let before = subs[0].anchors[0].leaving();

    move_points(&mut subs, &[(0, 1)], Vec2::new(5.0, -20.0));

    let moved = subs[0].anchors[1];
    assert_eq!(moved.at, Point::new(55.0, -20.0));
    assert_eq!(
        (moved.out, moved.back),
        (Vec2::new(10.0, 10.0), Vec2::new(-10.0, -10.0)),
        "the handles are offsets, so the point keeps its shape"
    );
    assert_eq!(
        moved.leaving(),
        Point::new(65.0, -10.0),
        "and the control points travelled with it"
    );
    assert_eq!(
        subs[0].anchors[0].leaving(),
        before,
        "the neighbour did not move, so its own handle must not have"
    );

    // A stale index is ignored rather than panicking.
    move_points(&mut subs, &[(9, 9), (0, 9)], Vec2::new(1.0, 1.0));
}

/// Deleting an anchor heals the path through the neighbours' own handles and drops a
/// subpath left with fewer than two anchors.
#[test]
fn deleting_a_point_heals_the_path_and_drops_a_subpath_that_cannot_draw() {
    use crate::kurbo::Vec2;
    let subs = vec![
        PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::smooth(Point::new(50.0, 40.0), Vec2::new(10.0, 0.0)),
                PenAnchor::smooth(Point::new(100.0, 0.0), Vec2::new(0.0, -10.0)),
                PenAnchor::corner(Point::new(150.0, 0.0)),
            ],
            closed: false,
        },
        // Two anchors: removing one leaves a point, which draws nothing.
        PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(200.0, 0.0)),
                PenAnchor::corner(Point::new(240.0, 0.0)),
            ],
            closed: false,
        },
    ];

    let left = remove_points(&subs, &[(0, 1), (1, 0)]);
    assert_eq!(left.len(), 1, "the two-anchor subpath is gone, not emptied");
    assert_eq!(subpath_lengths(&left), vec![3]);

    let healed = pen_path(&left);
    let PathEl::CurveTo(c1, c2, end) = healed.elements()[1] else {
        panic!("the heal should curve: {:?}", healed.elements());
    };
    assert_eq!(c1, Point::new(0.0, 0.0), "a corner leaves straight");
    assert_eq!(
        c2,
        Point::new(100.0, 10.0),
        "into the survivor's own handle"
    );
    assert_eq!(end, Point::new(100.0, 0.0));

    assert!(remove_points(&subs, &[(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)]).is_empty());
}

/// Dragging a handle within the retract radius snaps it onto its anchor, since
/// `is_corner` needs exact zeros; the mirror collapses the other handle too.
#[test]
fn a_handle_dragged_near_its_anchor_retracts_and_makes_a_corner() {
    use crate::kurbo::Vec2;
    let at = Point::new(50.0, 50.0);
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::smooth(at, Vec2::new(20.0, 0.0)),
            PenAnchor::corner(Point::new(100.0, 0.0)),
        ],
        closed: false,
    }];

    // Just outside the radius: still smooth.
    pull_handle(
        &mut subs,
        (0, 1),
        Side::Out,
        at + Vec2::new(8.0, 0.0),
        false,
        7.0,
        0.0,
    );
    assert!(!subs[0].anchors[1].is_corner());
    assert_eq!(subs[0].anchors[1].out, Vec2::new(8.0, 0.0));

    pull_handle(
        &mut subs,
        (0, 1),
        Side::Out,
        at + Vec2::new(5.0, 2.0),
        false,
        7.0,
        0.0,
    );
    assert!(subs[0].anchors[1].is_corner());
    let path = pen_path(&subs);
    assert!(
        path.elements()[1..]
            .iter()
            .all(|el| matches!(el, PathEl::LineTo(_))),
        "a retracted anchor between two corners leaves two lines: {:?}",
        path.elements()
    );

    // Alt breaks the pair, so a one-sided retract stays possible.
    let mut broken = vec![PenSubpath {
        anchors: vec![PenAnchor::smooth(at, Vec2::new(20.0, 0.0))],
        closed: false,
    }];
    pull_handle(&mut broken, (0, 0), Side::Out, at, true, 7.0, 0.0);
    assert_eq!(broken[0].anchors[0].out, Vec2::ZERO);
    assert_eq!(
        broken[0].anchors[0].back,
        Vec2::new(-20.0, 0.0),
        "the incoming handle is untouched"
    );
    assert!(!broken[0].anchors[0].is_corner());
}

/// Align and distribute over points use the extent the points themselves occupy.
#[test]
fn aligning_and_distributing_points_works_on_their_own_extent() {
    use crate::kurbo::Vec2;
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::smooth(Point::new(0.0, 10.0), Vec2::new(5.0, 5.0)),
            PenAnchor::corner(Point::new(30.0, 40.0)),
            PenAnchor::corner(Point::new(90.0, 70.0)),
            // Not selected, so nothing below may touch it.
            PenAnchor::corner(Point::new(45.0, 200.0)),
        ],
        closed: false,
    }];
    let picked = [(0, 0), (0, 1), (0, 2)];

    align_points(&mut subs, &picked, Axis::Y, Edge::Min);
    let ys: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.y).collect();
    assert_eq!(
        ys,
        vec![10.0, 10.0, 10.0, 200.0],
        "to the topmost, and only the three"
    );
    assert_eq!(
        subs[0].anchors[0].leaving(),
        Point::new(5.0, 15.0),
        "the handles travelled with the anchor"
    );

    // X is untouched by a Y align, so distribute has something to space.
    distribute_points(&mut subs, &picked, Axis::X);
    let xs: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.x).collect();
    assert_eq!(
        xs,
        vec![0.0, 45.0, 90.0, 45.0],
        "evenly between the two extremes, which stay put"
    );

    let before: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();
    distribute_points(&mut subs, &picked[..2], Axis::X);
    align_points(&mut subs, &picked[..1], Axis::X, Edge::Max);
    assert_eq!(
        subs[0].anchors.iter().map(|a| a.at).collect::<Vec<_>>(),
        before
    );
}

/// `rotate_points` carries each handle through the transform via its control point, so
/// a zero-length handle stays exactly zero; a quarter turn about an off-origin pivot.
///
/// flip: `rotate_points` reduced to `a.at = t * a.at` → fails at "the handle turned with its anchor"
#[test]
fn rotating_points_carries_their_handles_round_with_them() {
    use crate::kurbo::Vec2;
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::smooth(Point::new(20.0, 10.0), Vec2::new(5.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 10.0)),
            // Not selected.
            PenAnchor::corner(Point::new(50.0, 50.0)),
        ],
        closed: false,
    }];
    let picked = [(0, 0), (0, 1)];
    let pivot = Point::new(10.0, 10.0);

    rotate_points(&mut subs, &picked, pivot, std::f64::consts::FRAC_PI_2);

    let near = |a: Point, b: Point| (a - b).hypot() < 1e-9;
    assert!(
        near(subs[0].anchors[0].at, Point::new(10.0, 20.0)),
        "the anchor turned about the pivot: {:?}",
        subs[0].anchors[0].at
    );
    assert!(
        near(subs[0].anchors[0].leaving(), Point::new(10.0, 25.0)),
        "the handle turned with its anchor: {:?}",
        subs[0].anchors[0].out
    );
    assert!(
        (subs[0].anchors[0].out.hypot() - 5.0).abs() < 1e-9,
        "and kept its length: {}",
        subs[0].anchors[0].out.hypot()
    );
    assert_eq!(
        (subs[0].anchors[1].out, subs[0].anchors[1].back),
        (Vec2::ZERO, Vec2::ZERO),
        "a zero-length handle must stay at zero rather than at a rounding error"
    );
    assert_eq!(
        subs[0].anchors[2].at,
        Point::new(50.0, 50.0),
        "and the unselected anchor did not move"
    );
}

/// `space_points` lays the run out from the first point on the axis, whatever order
/// the selection was clicked in (0 / 10 / 40 at gap 5 gives 0 / 5 / 10).
///
/// flip: `order` reversed → fails at the "leftmost stays" assertion
#[test]
fn spacing_points_holds_the_first_one_along_the_axis() {
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(10.0, 0.0)),
            PenAnchor::corner(Point::new(40.0, 0.0)),
            // Not selected.
            PenAnchor::corner(Point::new(90.0, 0.0)),
        ],
        closed: false,
    }];
    // Scrambled on purpose: the axis decides the order.
    let picked = [(0, 2), (0, 0), (0, 1)];

    space_points(&mut subs, &picked, Axis::X, 5.0);

    let xs: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.x).collect();
    assert_eq!(
        xs,
        vec![0.0, 5.0, 10.0, 90.0],
        "the leftmost stays and the rest come to it, five apart"
    );

    // Fewer than two points, or a NaN gap, are no-ops.
    let before = xs;
    space_points(&mut subs, &picked[..1], Axis::X, 5.0);
    space_points(&mut subs, &picked, Axis::X, f64::NAN);
    assert_eq!(
        subs[0].anchors.iter().map(|a| a.at.x).collect::<Vec<_>>(),
        before
    );
}

/// Making a corner smooth mirrors the longer handle of a shaped anchor, or derives an
/// exactly mirrored pair from the chord `next − prev` at a third of the shorter leg.
#[test]
fn making_a_point_smooth_mirrors_a_handle_or_derives_one_from_the_neighbours() {
    use crate::kurbo::Vec2;
    // Legs of different lengths (10 across, 20 down) expose a handle taken from the wrong one.
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(10.0, 0.0)),
            PenAnchor::corner(Point::new(10.0, 20.0)),
        ],
        closed: false,
    }];
    set_smoothness(&mut subs, &[(0, 1)], true);
    let mid = subs[0].anchors[1];
    assert!(
        mid.is_smooth(),
        "the pair has to mirror *exactly*, or the next drag breaks it: {mid:?}"
    );
    let dir = Vec2::new(10.0, 20.0) / Vec2::new(10.0, 20.0).hypot();
    let want_out = dir * (10.0 / 3.0);
    assert!(
        (mid.out - want_out).hypot() < 1e-9,
        "out {:?} wanted {want_out:?}",
        mid.out
    );
    assert_eq!(subs[0].anchors[0].out, Vec2::ZERO);
    assert_eq!(subs[0].anchors[2].out, Vec2::ZERO);

    let mut broken = vec![PenSubpath {
        anchors: vec![PenAnchor {
            at: Point::ZERO,
            out: Vec2::new(9.0, 0.0),
            back: Vec2::new(0.0, 2.0),
            radius: 4.0,
        }],
        closed: false,
    }];
    set_smoothness(&mut broken, &[(0, 0)], true);
    let a = broken[0].anchors[0];
    assert_eq!(a.out, Vec2::new(9.0, 0.0), "the longer handle survives");
    assert_eq!(a.back, Vec2::new(-9.0, 0.0), "and the other mirrors it");
    assert_eq!(
        a.radius, 4.0,
        "the radius is left alone — it already draws nothing on a smooth anchor"
    );

    set_smoothness(&mut broken, &[(0, 0)], false);
    let a = broken[0].anchors[0];
    assert_eq!((a.out, a.back), (Vec2::ZERO, Vec2::ZERO));
    assert_eq!(a.radius, 4.0);
}

/// An open subpath's endpoint gets one handle and is not `is_smooth()`; closing the
/// subpath gives it two. Both `smoothed_handles` branches are driven (derived and
/// already-shaped), each end separately.
///
/// flip: delete either one-sided case of the already-shaped arm → fails at its own end's assertion
#[test]
fn an_open_subpaths_endpoint_smooths_on_the_side_it_has() {
    use crate::kurbo::Vec2;
    let anchors = vec![
        PenAnchor::corner(Point::new(0.0, 0.0)),
        PenAnchor::corner(Point::new(30.0, 0.0)),
        PenAnchor::corner(Point::new(30.0, 30.0)),
    ];
    let mut open = vec![PenSubpath {
        anchors: anchors.clone(),
        closed: false,
    }];
    set_smoothness(&mut open, &[(0, 0)], true);
    let end = open[0].anchors[0];
    assert_eq!(end.back, Vec2::ZERO, "there is no segment behind it");
    assert_eq!(end.out, Vec2::new(10.0, 0.0), "a third of its one leg");
    assert!(!end.is_smooth(), "and an end is not a joint");

    let mut closed = vec![PenSubpath {
        anchors,
        closed: true,
    }];
    set_smoothness(&mut closed, &[(0, 0)], true);
    let joint = closed[0].anchors[0];
    assert!(
        joint.is_smooth() && joint.back.hypot() > 0.0,
        "a closed subpath has no ends: {joint:?}"
    );

    // A lone anchor is left as it was.
    let mut lone = vec![PenSubpath {
        anchors: vec![PenAnchor::corner(Point::new(5.0, 5.0))],
        closed: false,
    }];
    set_smoothness(&mut lone, &[(0, 0)], true);
    assert_eq!(lone[0].anchors[0].at, Point::new(5.0, 5.0));
    assert_eq!(lone[0].anchors[0].out, Vec2::ZERO);

    // Endpoints that already carry a handle take the already-shaped branch.
    let mut shaped = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 0.0)),
        ],
        closed: false,
    }];
    shaped[0].anchors[0].out = Vec2::new(10.0, 0.0);
    assert_eq!(shaped[0].anchors[0].back, Vec2::ZERO);
    set_smoothness(&mut shaped, &[(0, 0)], true);
    let end = shaped[0].anchors[0];
    assert_eq!(
        end.out,
        Vec2::new(10.0, 0.0),
        "the handle that was aimed on purpose is kept"
    );
    assert_eq!(
        end.back,
        Vec2::ZERO,
        "and no second one is invented over a segment that does not exist"
    );
    assert!(!end.is_smooth(), "an end is still not a joint");

    // The other end, where the missing side is `out`.
    let mut tail = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 0.0)),
        ],
        closed: false,
    }];
    tail[0].anchors[1].back = Vec2::new(-10.0, 0.0);
    assert_eq!(tail[0].anchors[1].out, Vec2::ZERO);
    set_smoothness(&mut tail, &[(0, 1)], true);
    let end = tail[0].anchors[1];
    assert_eq!((end.back, end.out), (Vec2::new(-10.0, 0.0), Vec2::ZERO));
    assert!(!end.is_smooth());
}

/// `pen_anchors` and `geometry::anchor_runs` number a path's anchors and segments alike,
/// since `corner_radii` is written by one and read by the other.
#[test]
fn the_app_and_core_number_a_paths_anchors_alike() {
    let mut cases: Vec<BezPath> = Vec::new();
    let mut a = BezPath::new();
    a.move_to((0.0, 0.0));
    a.line_to((10.0, 0.0));
    a.curve_to((15.0, 5.0), (15.0, 5.0), (10.0, 10.0));
    a.line_to((0.0, 0.0));
    a.close_path();
    cases.push(a);
    // A `ClosePath` with an implied final edge.
    let mut b = BezPath::new();
    b.move_to((0.0, 0.0));
    b.line_to((10.0, 0.0));
    b.line_to((10.0, 10.0));
    b.close_path();
    cases.push(b);
    // Several subpaths, a quad, and an open run.
    let mut c = BezPath::new();
    c.move_to((0.0, 0.0));
    c.quad_to((5.0, 5.0), (10.0, 0.0));
    c.move_to((50.0, 50.0));
    c.line_to((60.0, 50.0));
    c.line_to((60.0, 60.0));
    c.close_path();
    cases.push(c);
    cases.push(BezPath::new());

    for path in &cases {
        let subs = pen_anchors(path, &[]);
        let runs = crate::geometry::anchor_runs(path);
        let mine: Vec<Point> = subs
            .iter()
            .flat_map(|s| s.anchors.iter().map(|a| a.at))
            .collect();
        let theirs: Vec<Point> = runs.iter().flat_map(|r| r.points.clone()).collect();
        assert_eq!(mine, theirs, "for {:?}", path.elements());
        assert_eq!(mine.len(), crate::geometry::anchor_count(path));

        // Segments are named by the anchor they leave: one per anchor when
        // closed, one fewer when open.
        assert_eq!(subs.len(), runs.len(), "for {:?}", path.elements());
        for (i, (sub, run)) in subs.iter().zip(runs.iter()).enumerate() {
            let want = match sub.closed {
                true => sub.anchors.len(),
                false => sub.anchors.len().saturating_sub(1),
            };
            assert_eq!(run.segs.len(), want, "subpath {i} segment count");
            for (j, &seg) in run.segs.iter().enumerate() {
                let (from, to) =
                    segment_ends(&subs, (i, j)).unwrap_or_else(|| panic!("segment {i}.{j}"));
                assert_eq!(from, (i, j), "a segment is named by the anchor it leaves");
                assert_eq!(seg.start(), sub.anchors[from.1].at, "subpath {i} seg {j}");
                assert_eq!(seg.end(), sub.anchors[to.1].at, "subpath {i} seg {j}");
            }
            // One past the last segment names nothing.
            assert!(
                segment_ends(&subs, (i, run.segs.len())).is_none(),
                "subpath {i} claims a segment it does not have"
            );
        }
    }

    let mut subs = pen_anchors(&cases[0], &[0.0, 4.0, 7.0]);
    assert_eq!(subs[0].anchors[1].radius, 4.0);
    assert_eq!(subs[0].anchors[2].radius, 7.0);
    assert_eq!(pen_radii(&subs), vec![0.0, 4.0, 7.0]);
    // Trailing zeros are trimmed so equal shapes serialize alike.
    subs[0].anchors[1].radius = 0.0;
    subs[0].anchors[2].radius = 0.0;
    assert!(pen_radii(&subs).is_empty());
}

#[test]
fn create_line_endpoints_map_to_world() {
    let mut ids = IdSource::new(3);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let ab = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: ab,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((5.0, 7.0))),
        name: None,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let pw = res.world_transform(ab).unwrap();

    let line = ids.mint();
    let (a, b) = (Point::new(20.0, 30.0), Point::new(120.0, 80.0));
    doc.apply(&create_line(line, ab, 0, pw, a, b, Color::BLACK, 2.0))
        .unwrap();
    let res = Resolved::rebuild(&doc);
    let world = res.world_transform(line).unwrap();
    let NodeKind::Line { end } = doc.get(line).unwrap().kind() else {
        panic!("expected a line");
    };
    let w_start = world * Point::ZERO;
    let w_end = world * *end;
    assert!((w_start.x - a.x).abs() < 1e-9 && (w_start.y - a.y).abs() < 1e-9);
    assert!((w_end.x - b.x).abs() < 1e-9 && (w_end.y - b.y).abs() < 1e-9);
}

#[test]
fn rotate_about_pivot_moves_points_correctly() {
    let (mut doc, res, rect) = artboard_with_rect();
    let tx = rotate_node(
        &doc,
        &res,
        rect,
        Point::new(100.0, 100.0),
        std::f64::consts::FRAC_PI_2,
    );
    doc.apply(&tx).unwrap();
    let res2 = Resolved::rebuild(&doc);
    let w = res2.world_transform(rect).unwrap();
    let p = w * Point::new(50.0, 0.0);
    assert!((p.x - 100.0).abs() < 1e-6, "x={}", p.x);
    assert!((p.y - 150.0).abs() < 1e-6, "y={}", p.y);
}

/// A rotation under a skewed parent stores no scale in the transform; the world
/// placement is asserted as a control, and an unskewed parent as the second half.
///
/// flip: `rotate_node` writes `new_local` without the scale split → fails at "a rotation under a skewed parent stores no scale"
#[test]
fn a_rotation_under_a_skewed_parent_leaves_the_scale_in_the_geometry() {
    let scale_after = |lean: f64| {
        let mut ids = IdSource::new(0x5CE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (group, rect) = (ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                // A 45° lean at 1.0, none at 0.0.
                transform: Some(Affine::new([1.0, 0.0, lean, 1.0, 0.0, 0.0])),
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("the fixture");
        let res = Resolved::rebuild(&doc);
        let before = res.world_transform(rect).expect("a world basis");
        let probe = before * Point::new(100.0, 0.0);

        let pivot = Point::new(50.0, 50.0);
        let angle = std::f64::consts::FRAC_PI_6;
        let tx = rotate_node(&doc, &res, rect, pivot, angle);
        doc.apply(&tx).expect("the rotation applies");
        let res = Resolved::rebuild(&doc);

        // Read the corner through the node's own `size`: the split moves scale
        // into the geometry, so the same local coordinate names a different place.
        let node = doc.get(rect).expect("a live node");
        let after = res.world_transform(rect).expect("a world basis");
        let NodeKind::Rect { size, .. } = node.kind() else {
            panic!("a rect");
        };
        let want = turn_about(pivot, angle) * probe;
        let got = after * Point::new(size.width, 0.0);
        (Basis::of(node.transform()).scale, (got - want).hypot())
    };

    let (skewed, moved) = scale_after(1.0);
    assert!(
        moved < 1e-6,
        "the control: the rotation still puts the point where it promised, \
         off by {moved}"
    );
    assert!(
        (skewed.x - 1.0).abs() < 1e-6 && (skewed.y - 1.0).abs() < 1e-6,
        "a rotation under a skewed parent stores no scale: {skewed:?}"
    );

    let (upright, moved) = scale_after(0.0);
    assert!(moved < 1e-6, "the upright control's placement: {moved}");
    assert!(
        (upright.x - 1.0).abs() < 1e-6 && (upright.y - 1.0).abs() < 1e-6,
        "and neither does one under an unskewed parent — which is the arm that \
         always worked: {upright:?}"
    );
}

// --- transforming a whole selection ------------------------------------

/// Two sibling 20×20 squares at x = 0 and x = 80, whose union is 100×20.
fn two_squares() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(0x5E1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
    let square = || NodeKind::Rect {
        size: Size::new(20.0, 20.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: a,
            parent: ab,
            index: 0,
            kind: square(),
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: b,
            parent: ab,
            index: 1,
            kind: square(),
            transform: Some(Affine::translate((80.0, 0.0))),
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, a, b)
}

fn union_of(res: &Resolved, ids: &[NodeId]) -> Rect {
    ids.iter()
        .filter_map(|id| res.world_bounds(*id))
        .reduce(|x, y| x.union(y))
        .unwrap()
}

/// A selection resize scales both the gaps between members and each member's geometry.
#[test]
fn resizing_a_selection_scales_its_members_and_the_gaps_between_them() {
    let (mut doc, res, a, b) = two_squares();
    let roots = [a, b];
    let union = union_of(&res, &roots);
    assert_eq!((union.width(), union.height()), (100.0, 20.0));

    doc.apply(&resize_selection(
        &doc,
        &res,
        &roots,
        SelectionBox::upright(union),
        Handle::BottomRight,
        Point::new(200.0, 20.0),
        Resize::geometry(false, false),
    ))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    for (id, want_x, want_w) in [(a, 0.0, 40.0), (b, 160.0, 40.0)] {
        let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
            panic!("not a rect");
        };
        assert!(
            (size.width - want_w).abs() < 1e-9 && (size.height - 20.0).abs() < 1e-9,
            "{id:?} geometry {size:?}, want {want_w}x20"
        );
        let origin = res.world_transform(id).unwrap() * Point::ZERO;
        assert!(
            (origin.x - want_x).abs() < 1e-9,
            "{id:?} at {origin:?}, want x {want_x}"
        );
    }
    let after = union_of(&res, &roots);
    assert!(
        (after.width() - 200.0).abs() < 1e-9 && (after.height() - 20.0).abs() < 1e-9,
        "the union box came out {after:?}"
    );
}

/// A selection resize holds the opposite corner still.
#[test]
fn resizing_a_selection_holds_the_opposite_corner_still() {
    let (mut doc, res, a, b) = two_squares();
    let roots = [a, b];
    let union = union_of(&res, &roots);
    doc.apply(&resize_selection(
        &doc,
        &res,
        &roots,
        SelectionBox::upright(union),
        Handle::BottomRight,
        Point::new(50.0, 10.0),
        Resize::geometry(false, false),
    ))
    .unwrap();
    let after = union_of(&Resolved::rebuild(&doc), &roots);
    assert!(
        (after.min_x() - union.min_x()).abs() < 1e-9
            && (after.min_y() - union.min_y()).abs() < 1e-9,
        "the held corner moved: {union:?} -> {after:?}"
    );
}

/// A selection turns about the shared centre, not each member's own.
#[test]
fn rotating_a_selection_turns_it_about_the_shared_centre() {
    let (mut doc, res, a, b) = two_squares();
    let roots = [a, b];
    let union = union_of(&res, &roots);
    let pivot = union.center();

    doc.apply(&rotate_selection(
        &doc,
        &res,
        &roots,
        pivot,
        std::f64::consts::FRAC_PI_2,
    ))
    .unwrap();

    let res = Resolved::rebuild(&doc);
    let after = union_of(&res, &roots);
    assert!(
        (after.width() - 20.0).abs() < 1e-9 && (after.height() - 100.0).abs() < 1e-9,
        "the box did not turn: {after:?}"
    );
    assert!(
        (after.center().x - pivot.x).abs() < 1e-9 && (after.center().y - pivot.y).abs() < 1e-9,
        "the selection drifted off its pivot: {:?} -> {:?}",
        pivot,
        after.center()
    );
    for id in roots {
        let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
            panic!("not a rect");
        };
        assert_eq!((size.width, size.height), (20.0, 20.0));
    }
    let (ca, cb) = (
        res.world_bounds(a).unwrap().center(),
        res.world_bounds(b).unwrap().center(),
    );
    assert!(
        (ca.y - cb.y).abs() > 50.0,
        "the two squares did not separate along y: {ca:?} {cb:?}"
    );
}

/// A member at an odd angle under a non-uniform selection scale leans, and the selection
/// lands exactly where the box says; asserted on world bounds, not node fields.
#[test]
fn a_selection_member_at_an_odd_angle_leans_instead_of_evening_out() {
    let (mut doc, _, a, b) = two_squares();
    doc.apply(&Transaction(vec![Operation::SetTransform {
        id: b,
        transform: Affine::translate((80.0, 0.0)) * Affine::rotate(30f64.to_radians()),
    }]))
    .unwrap();
    let res2 = Resolved::rebuild(&doc);
    let roots = [a, b];
    let union = union_of(&res2, &roots);
    let before = res2.world_bounds(b).unwrap();

    doc.apply(&resize_selection(
        &doc,
        &res2,
        &roots,
        SelectionBox::upright(union),
        Handle::BottomRight,
        Point::new(union.max_x() * 3.0, union.max_y()),
        Resize::geometry(false, false),
    ))
    .unwrap();
    let after = Resolved::rebuild(&doc);

    let hold = Point::new(union.min_x(), union.min_y());
    let want = Rect::new(
        hold.x + (before.min_x() - hold.x) * 3.0,
        before.min_y(),
        hold.x + (before.max_x() - hold.x) * 3.0,
        before.max_y(),
    );
    let got = after.world_bounds(b).unwrap();
    for (g, w) in [
        (got.min_x(), want.min_x()),
        (got.min_y(), want.min_y()),
        (got.max_x(), want.max_x()),
        (got.max_y(), want.max_y()),
    ] {
        assert!(
            (g - w).abs() < 1e-6,
            "the turned member did not follow the box: {got:?} vs {want:?}"
        );
    }

    // It leans rather than stretching along its own axes.
    let skew = build::Orientation::of(after.world_transform(b).unwrap()).skew;
    assert!(
        skew.abs() > 1f64.to_radians(),
        "the turned member came out unsheared: {}°",
        skew.to_degrees()
    );

    // The axis-aligned member took the scale as pure geometry.
    let NodeKind::Rect { size: flat, .. } = doc.get(a).unwrap().kind() else {
        panic!("not a rect");
    };
    assert!(
        flat.width > flat.height * 1.5,
        "the axis-aligned member should have stretched: {flat:?}"
    );
    assert!(
        build::Orientation::of(after.world_transform(a).unwrap())
            .skew
            .abs()
            < 1e-9,
        "an axis-aligned member must not pick up a lean"
    );
}

/// Ctrl-dragging a side leans the box and holds the side facing it still.
#[test]
fn skewing_a_side_holds_the_one_facing_it() {
    let (mut doc, res, a, _) = two_squares();
    let before = res.world_bounds(a).unwrap();

    doc.apply(&skew_to_handle(
        &doc,
        &res,
        a,
        Rect::new(0.0, 0.0, 20.0, 20.0),
        Handle::Top,
        SideDrag {
            press: Point::new(before.center().x, before.min_y()),
            at: Point::new(before.center().x + 20.0, before.min_y()),
        },
        false,
    ))
    .unwrap();
    let after = Resolved::rebuild(&doc);

    // −45, not 45: `skewX` displaces +x with +y and y points down, matching the
    // inspector's Skew field and the SVG writer.
    let o = build::Orientation::of(after.world_transform(a).unwrap());
    assert!(
        (o.skew.to_degrees() + 45.0).abs() < 1e-6,
        "a top side dragged one height right is a −45° lean, not {}°",
        o.skew.to_degrees()
    );
    let got = after.world_bounds(a).unwrap();
    assert!(
        (got.max_y() - before.max_y()).abs() < 1e-6,
        "the held side moved: {before:?} -> {got:?}"
    );
    assert!(
        got.width() > before.width() + 10.0,
        "the box did not lean at all: {before:?} -> {got:?}"
    );
}

/// A side grabbed off its midpoint leans by the drag delta from the press, not by the
/// grab's offset from the box centre.
///
/// flip: measure from the centre (`p.x - c.x`) instead of `p.x - press.x` → fails at the no-lean assertion
#[test]
fn a_side_grabbed_off_centre_does_not_lean_until_the_pointer_moves() {
    let mut ids = IdSource::new(7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let id = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(200.0, 120.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }]))
    .expect("the reported rectangle");
    let res = Resolved::rebuild(&doc);
    let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
    let press = Point::new(160.5, 0.0);

    let lean_after = |dx: f64| {
        let tx = skew_to_handle(
            &doc,
            &res,
            id,
            box_,
            Handle::Top,
            SideDrag {
                press,
                at: Point::new(press.x + dx, press.y),
            },
            false,
        );
        let mut d = doc.clone();
        if !tx.0.is_empty() {
            d.apply(&tx).expect("the skew applies");
        }
        let r = Resolved::rebuild(&d);
        build::Orientation::of(r.world_transform(id).unwrap())
            .skew
            .to_degrees()
    };

    assert!(
        lean_after(0.0).abs() < 1e-9,
        "pressing without moving leant it {}° — this is the reported jump",
        lean_after(0.0)
    );

    let small = lean_after(1.0);
    let want = (1.0f64 / -120.0).atan().to_degrees();
    assert!(
        (small - want).abs() < 1e-6,
        "a 1-unit drag should lean {want}°, got {small}°"
    );

    let full = lean_after(60.5);
    assert!(
        (full + 26.76).abs() < 0.01,
        "dragging the old grab-offset should still reach the reported −26.76°, \
         got {full}° — the angle is not gone, it just takes a drag now"
    );
}

/// A selection leans as one parallelogram even with a turned member; compared on each
/// member's unit-square-to-world map, since the split divides scale between matrix and size.
#[test]
fn a_leaning_selection_carries_a_turned_member_exactly() {
    let (mut doc, _, a, b) = two_squares();
    doc.apply(&Transaction(vec![Operation::SetTransform {
        id: b,
        transform: Affine::translate((80.0, 0.0)) * Affine::rotate(37f64.to_radians()),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let roots = [a, b];
    let union = union_of(&res, &roots);
    let placement = |d: &Document, r: &Resolved, id: NodeId| {
        let NodeKind::Rect { size, .. } = d.get(id).unwrap().kind() else {
            panic!("not a rect")
        };
        r.world_transform(id).unwrap() * Affine::scale_non_uniform(size.width, size.height)
    };
    let before: Vec<Affine> = roots.iter().map(|id| placement(&doc, &res, *id)).collect();

    let shear = side_shear(
        union,
        Handle::Top,
        SideDrag {
            press: Point::new(union.center().x, union.min_y()),
            at: Point::new(union.center().x + union.height(), union.min_y()),
        },
        false,
    )
    .expect("a lean this box can take");
    doc.apply(&skew_selection(&doc, &res, &roots, shear))
        .unwrap();
    let after = Resolved::rebuild(&doc);

    let want_shear = Affine::translate((0.0, union.max_y()))
        * build::skew_x(-45f64.to_radians())
        * Affine::translate((0.0, -union.max_y()));
    for (id, was) in roots.iter().zip(before) {
        let want = want_shear * was;
        let got = placement(&doc, &after, *id);
        for (g, w) in got.as_coeffs().iter().zip(want.as_coeffs().iter()) {
            assert!(
                (g - w).abs() < 1e-6,
                "member landed off the shared shear: {:?} vs {:?}",
                got.as_coeffs(),
                want.as_coeffs()
            );
        }
    }
}

/// A lean preserves the shape's area.
#[test]
fn a_lean_does_not_change_how_much_shape_there_is() {
    let (mut doc, res, a, _) = two_squares();
    let area = |d: &Document| {
        let NodeKind::Rect { size, .. } = d.get(a).unwrap().kind() else {
            panic!("not a rect")
        };
        let det = d.get(a).unwrap().transform().determinant().abs();
        size.width * size.height * det
    };
    let before = area(&doc);

    doc.apply(&skew_to_handle(
        &doc,
        &res,
        a,
        Rect::new(0.0, 0.0, 20.0, 20.0),
        Handle::Right,
        SideDrag {
            press: Point::new(
                res.world_bounds(a).unwrap().max_x(),
                res.world_bounds(a).unwrap().center().y,
            ),
            at: Point::new(res.world_bounds(a).unwrap().max_x(), 30.0),
        },
        false,
    ))
    .unwrap();

    assert!(
        (area(&doc) - before).abs() < 1e-6,
        "the lean changed the area: {before} -> {}",
        area(&doc)
    );
}

// --- the Scale tool ----------------------------------------------------

/// A rounded, stroked, dotted rect under an artboard, for scaling.
fn artboard_with_a_decorated_rect() -> (Document, Resolved, NodeId) {
    let (mut doc, _, rect) = artboard_with_rect();
    doc.apply(&Transaction(vec![
        Operation::SetGeometry {
            id: rect,
            geometry: GeometryPatch::CornerRadii(RoundedRectRadii::new(4.0, 4.0, 8.0, 8.0)),
        },
        Operation::SetStrokes {
            id: rect,
            strokes: crate::keyed_by_position([Stroke {
                width: 2.0,
                dashes: vec![0.0, 6.0],
                dash_offset: 3.0,
                sides: crate::StrokeSides::Custom {
                    top: 1.0,
                    right: 0.0,
                    bottom: 3.0,
                    left: 2.0,
                },
                ..Default::default()
            }]),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, rect)
}

fn radii(doc: &Document, id: NodeId) -> RoundedRectRadii {
    let NodeKind::Rect { corner_radii, .. } = doc.get(id).unwrap().kind() else {
        panic!("not a rect");
    };
    *corner_radii
}

/// A photographic scale takes stroke width, corner radii, dashes and per-side widths
/// with it, and a geometry resize leaves them; asserted as a pair so an unread flag fails.
#[test]
fn scaling_takes_the_stroke_radii_and_type_that_a_resize_leaves_alone() {
    for (scaling, want) in [(Scaling::Geometry, 2.0), (Scaling::Photographic, 4.0)] {
        let (mut doc, res, rect) = artboard_with_a_decorated_rect();
        // 50 -> 100 on both axes: a clean doubling.
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize {
                scaling,
                ..Default::default()
            },
        ))
        .unwrap();

        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!()
        };
        assert_eq!(size, &Size::new(100.0, 100.0), "{scaling:?}: the box");
        let stroke = &doc.get(rect).unwrap().paint().strokes[0];
        assert_eq!(stroke.width, want, "{scaling:?}: stroke width");
        let s = want / 2.0;
        // Each corner scales from its own radius.
        assert_eq!(radii(&doc, rect).top_left, 4.0 * s, "{scaling:?}: top-left");
        assert_eq!(
            radii(&doc, rect).bottom_right,
            8.0 * s,
            "{scaling:?}: bottom-right"
        );
        assert_eq!(stroke.dashes, vec![0.0, 6.0 * s], "{scaling:?}: dashes");
        assert_eq!(stroke.dash_offset, 3.0 * s, "{scaling:?}: dash offset");
        // A side of 0 stays 0.
        assert_eq!(
            stroke.sides,
            crate::StrokeSides::Custom {
                top: s,
                right: 0.0,
                bottom: 3.0 * s,
                left: 2.0 * s,
            },
            "{scaling:?}: per-side widths"
        );
    }
}

/// Scaling a square-cornered, unstroked rect emits no radius or stroke operation.
#[test]
fn scaling_a_square_cornered_rect_writes_no_radius_operation() {
    let (doc, res, rect) = artboard_with_rect();
    let tx = resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::BottomRight,
        Point::new(200.0, 200.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    );
    assert!(
        !tx.0.iter().any(|op| matches!(
            op,
            Operation::SetGeometry {
                geometry: GeometryPatch::CornerRadii(_),
                ..
            }
        )),
        "a square corner needs no radius patch: {tx:?}"
    );
    assert!(
        !tx.0
            .iter()
            .any(|op| matches!(op, Operation::SetStrokes { .. })),
        "an unstroked layer needs no stroke patch: {tx:?}"
    );
}

/// A photographic scale takes a polygon's corner radius with it, and a geometry
/// resize leaves it, as it does a rect's.
#[test]
fn scaling_takes_a_polygons_radius_that_a_resize_leaves_alone() {
    for (scaling, want) in [(Scaling::Geometry, 5.0), (Scaling::Photographic, 10.0)] {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let poly = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: poly,
            parent: root,
            index: 0,
            kind: NodeKind::Polygon {
                size: Size::new(50.0, 50.0),
                sides: 6,
                corner_radius: 5.0,
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            poly,
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize {
                scaling,
                ..Default::default()
            },
        ))
        .unwrap();
        let NodeKind::Polygon {
            size,
            corner_radius,
            ..
        } = doc.get(poly).unwrap().kind()
        else {
            panic!()
        };
        assert_eq!(size, &Size::new(100.0, 100.0), "{scaling:?}: the box");
        assert_eq!(*corner_radius, want, "{scaling:?}: the radius");
    }
}

/// A non-uniform scale moves the scalars by the geometric mean of the two factors.
#[test]
fn a_non_uniform_scale_moves_the_scalars_by_the_geometric_mean() {
    let (mut doc, res, rect) = artboard_with_a_decorated_rect();
    doc.apply(&resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::BottomRight,
        Point::new(300.0, 150.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    ))
    .unwrap();
    let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
        panic!()
    };
    assert_eq!(size, &Size::new(200.0, 50.0), "the box took both factors");
    assert_eq!(
        doc.get(rect).unwrap().paint().strokes[0].width,
        4.0,
        "sqrt(4 * 1) = 2, so a 2px stroke becomes 4px"
    );
}

/// Scaling a group carries the treatment down to every leaf.
#[test]
fn scaling_a_group_scales_its_childrens_strokes_too() {
    let (mut doc, _, rect) = artboard_with_a_decorated_rect();
    let parent = doc.get(rect).unwrap().parent().expect("artboard");
    let mut ids = IdSource::new(99);
    let group = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: group,
        parent,
        index: 1,
        kind: NodeKind::Group,
        transform: None,
        name: None,
    }]))
    .unwrap();
    doc.apply(&Transaction(vec![Operation::Reparent {
        id: rect,
        new_parent: group,
        index: 0,
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let local = crate::local_box(&doc, &res, group).expect("group box");

    doc.apply(&resize_group(
        &doc,
        &res,
        group,
        local,
        Handle::BottomRight,
        Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    ))
    .unwrap();
    assert!(
        doc.get(rect).unwrap().paint().strokes[0].width > 2.5,
        "the child's stroke stayed at {}",
        doc.get(rect).unwrap().paint().strokes[0].width
    );
}

/// A group doubled under the Scale tool scales a card frame's contents with it, and
/// under a resize only the frame's box doubles.
///
/// flip: delete the `Scaling::Photographic` arm in `scale_subtree` → fails at the rect size under Scale
#[test]
fn a_frame_in_a_scaled_group_takes_its_contents_with_it_and_a_resize_does_not() {
    for (scaling, rect_size, rect_at, stroke) in [
        (
            Scaling::Photographic,
            Size::new(40.0, 20.0),
            Point::new(20.0, 20.0),
            4.0,
        ),
        (
            Scaling::Geometry,
            Size::new(20.0, 10.0),
            Point::new(10.0, 10.0),
            2.0,
        ),
    ] {
        let mut ids = IdSource::new(0xF4A);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (group, card, rect) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: card,
                parent: group,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 50.0),
                    corner_radii: RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: card,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: None,
            },
            Operation::SetStrokes {
                id: rect,
                strokes: crate::keyed_by_position([Stroke {
                    width: 2.0,
                    ..Default::default()
                }]),
            },
        ]))
        .expect("a frame may sit in a group");
        let res = Resolved::rebuild(&doc);
        let local = crate::local_box(&doc, &res, group).expect("group box");
        assert_eq!(local, Rect::new(0.0, 0.0, 100.0, 50.0), "the fixture");

        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(200.0, 100.0),
            Resize {
                scaling,
                ..Default::default()
            },
        ))
        .unwrap();
        let NodeKind::Artboard { size, .. } = doc.get(card).unwrap().kind() else {
            panic!("not a frame")
        };
        assert_eq!(
            size,
            &Size::new(200.0, 100.0),
            "{scaling:?}: the card's box"
        );
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!("not a rect")
        };
        assert_eq!(size, &rect_size, "{scaling:?}: the rect inside the card");
        assert_eq!(
            doc.get(rect).unwrap().transform().translation(),
            rect_at.to_vec2(),
            "{scaling:?}: where the rect sits in the card"
        );
        assert_eq!(
            doc.get(rect).unwrap().paint().strokes[0].width,
            stroke,
            "{scaling:?}: the rect's stroke"
        );
    }
}

/// A flex group dragged to twice its hugged size is resized by its box (rects untouched),
/// and the Scale tool also doubles the rects, padding and gap.
///
/// flip: delete `resize_layer`'s `laid_group` routing → fails at "the box" under resize
#[test]
fn a_group_with_a_layout_is_resized_by_its_box() {
    use crate::container::{Dimension, Display, Flex};
    for scaling in [Scaling::Geometry, Scaling::Photographic] {
        let mut ids = IdSource::new(0xF1E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (row, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let square = |id, index| Operation::CreateNode {
            id,
            parent: row,
            index,
            kind: NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: row,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
            square(a, 0),
            square(b, 1),
            Operation::SetDisplay {
                id: row,
                display: Some(Display::Flex(Flex {
                    column_gap: 10.0,
                    padding: [5.0; 4],
                    ..Default::default()
                })),
            },
        ]))
        .expect("the row");
        let res = Resolved::rebuild(&doc);
        assert_eq!(
            res.used_frame(row),
            Some(Size::new(60.0, 30.0)),
            "the fixture hugs"
        );

        doc.apply(&resize_layer(
            &doc,
            &res,
            row,
            Handle::BottomRight,
            Point::new(220.0, 160.0),
            Resize {
                scaling,
                ..Default::default()
            },
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let node = doc.get(row).unwrap();
        assert_eq!(
            (node.item().width, node.item().height),
            (Dimension::Px(120.0), Dimension::Px(60.0)),
            "{scaling:?}: the box"
        );
        assert_eq!(
            res.used_frame(row),
            Some(Size::new(120.0, 60.0)),
            "{scaling:?}: laid out"
        );
        assert_eq!(
            res.world_transform(row).map(|t| t.translation()),
            Some(Vec2::new(100.0, 100.0)),
            "{scaling:?}: the held corner stayed"
        );
        let (side, padding, gap) = match scaling {
            Scaling::Geometry => (20.0, 5.0, 10.0),
            Scaling::Photographic => (40.0, 10.0, 20.0),
        };
        let NodeKind::Rect { size, .. } = doc.get(a).unwrap().kind() else {
            panic!("not a rect")
        };
        assert_eq!(size, &Size::new(side, side), "{scaling:?}: the rects");
        let Some(Display::Flex(flex)) = node.display() else {
            panic!("the layout went")
        };
        assert_eq!(flex.padding, [padding; 4], "{scaling:?}: the padding");
        assert_eq!(flex.column_gap, gap, "{scaling:?}: the gap");
    }
}

/// A pinned child of a flex group doubled by the Scale tool stays flush with the scaled
/// box's corner, committed through `build::keep_insets` and `keep_flex_sizes`.
///
/// flip: `keep_insets` measures a laid group by its committed `used_frame` → fails at "flush with the scaled box's corner"
#[test]
fn a_pinned_child_of_a_flex_group_scaled_with_it_stays_pinned_to_the_scaled_box() {
    use crate::container::{Display, Flex};
    let mut ids = IdSource::new(0xF1F);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (row, a, b, pinned) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
    let square = |id, index, side| Operation::CreateNode {
        id,
        parent: row,
        index,
        kind: NodeKind::Rect {
            size: Size::new(side, side),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: row,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: Some(Affine::translate((100.0, 100.0))),
            name: None,
        },
        square(a, 0, 20.0),
        square(b, 1, 20.0),
        square(pinned, 2, 10.0),
        Operation::SetDisplay {
            id: row,
            display: Some(Display::Flex(Flex {
                column_gap: 10.0,
                padding: [5.0; 4],
                ..Default::default()
            })),
        },
        Operation::SetInsets {
            id: pinned,
            insets: crate::Insets {
                right: Some(crate::LengthPct::Px(0.0)),
                bottom: Some(crate::LengthPct::Px(0.0)),
                ..Default::default()
            },
        },
    ]))
    .expect("the row");
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(pinned),
        Some(Rect::new(150.0, 120.0, 160.0, 130.0)),
        "the fixture: pinned to the 60 × 30 box's corner"
    );

    let tx = resize_layer(
        &doc,
        &res,
        row,
        Handle::BottomRight,
        Point::new(220.0, 160.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    );
    let tx = crate::build::keep_insets(&doc, &res, tx);
    let tx = crate::build::keep_flex_sizes(&doc, &res, tx);
    doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&doc);
    assert_eq!(
        res.used_frame(row),
        Some(Size::new(120.0, 60.0)),
        "the box doubled"
    );
    assert_eq!(
        res.world_bounds(pinned),
        Some(Rect::new(200.0, 140.0, 220.0, 160.0)),
        "flush with the scaled box's corner"
    );
}

/// Auto-sized text takes the scaled font size but keeps its auto sizing mode.
#[test]
fn scaling_auto_sized_text_moves_its_type_size_but_not_its_sizing_mode() {
    let (mut doc, res, group, text) = group_with_auto_text();
    let local = crate::local_box(&doc, &res, group).expect("group box");
    doc.apply(&resize_group(
        &doc,
        &res,
        group,
        local,
        Handle::BottomRight,
        Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    ))
    .unwrap();
    let NodeKind::Text { sizing, style, .. } = doc.get(text).unwrap().kind() else {
        panic!("not text")
    };
    assert!(
        matches!(sizing, TextSizing::Auto),
        "auto text must keep its sizing mode: {sizing:?}"
    );
    assert!(
        style.font_size > 16.5,
        "the type size stayed at {}",
        style.font_size
    );
}

/// A photographic scale leaves a bound size at its literal 0, so the binding
/// still reads it as its own and the text draws its variable.
///
/// flip: `scaled_text`'s copy of the bound slots dropped → fails at *"still
/// bound"*, the size clamped to 1 and detached.
#[test]
fn scaling_a_text_keeps_its_bound_size() {
    use crate::variable::TEXT_SIZE;
    let (mut doc, _, group, text) = group_with_auto_text();
    let mut ids = IdSource::new(8);
    let (mode, var) = (ids.mint_mode(), ids.mint_var());
    doc.apply(&Transaction(vec![
        Operation::AddMode {
            collection: crate::variable::CollectionId::FIRST,
            mode: crate::Mode {
                id: mode,
                name: "Mode 1".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddVariable {
            var: crate::Variable {
                id: var,
                name: "type/size".into(),
                kind: crate::VarKind::Number,
                cells: [(mode, crate::Cell::Number(32.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .unwrap();
    let bind = crate::bind_text_ops(&doc, text, crate::variable::text_vars_at(TEXT_SIZE, var));
    doc.apply(&Transaction(bind)).unwrap();
    let res = Resolved::rebuild(&doc);
    let local = crate::local_box(&doc, &res, group).expect("group box");
    let tx = resize_group(
        &doc,
        &res,
        group,
        local,
        Handle::BottomRight,
        Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    );
    // The commit's pass, which judges the size written against the binding.
    let tx = crate::build::keep_text_bindings(&doc, tx);
    doc.apply(&tx).unwrap();
    let node = doc.get(text).unwrap();
    assert_eq!(node.text_vars()[TEXT_SIZE], Some(var), "still bound");
    let NodeKind::Text { style, .. } = node.kind() else {
        panic!("not text")
    };
    assert_eq!(style.font_size, 0.0, "the literal 0");
    let Some(NodeKind::Text { style, .. }) = doc.resolved_kind(text).map(|k| k.into_owned()) else {
        panic!("not text")
    };
    assert_eq!(style.font_size, 32.0, "the variable drawn");
}

/// A group with an auto-sized text layer and a rect beside it, so the group has
/// a box to scale.
fn group_with_auto_text() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let group = ids.mint();
    let text = ids.mint();
    let rect = ids.mint();
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: group,
            parent: root,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: group,
            index: 0,
            kind: NodeKind::Text {
                content: "Label".into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 16.0,
                    weight: 400,
                    italic: false,
                    line_height: Some(crate::Length::Em(1.2)),
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: group,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((0.0, 100.0))),
            name: None,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, group, text)
}

/// A scale of one writes no scalar operations; `Scaling::scalar` guards factors a few ulps off 1.
#[test]
fn a_scale_of_one_writes_no_scalar_operations() {
    let (doc, res, rect) = artboard_with_a_decorated_rect();
    let tx = resize_to_handle(
        &doc,
        &res,
        rect,
        Handle::BottomRight,
        Point::new(150.0, 150.0),
        Resize {
            scaling: Scaling::Photographic,
            ..Default::default()
        },
    );
    assert!(
        !tx.0
            .iter()
            .any(|op| matches!(op, Operation::SetStrokes { .. })),
        "an unchanged box must not rewrite the strokes: {tx:?}"
    );
}

// --- segments: insert, remove, move, bend ------------------------------

/// A closed run with one curved side, as `pen_anchors` would hand it over.
fn quad_with_a_curve() -> Vec<PenSubpath> {
    use crate::kurbo::Vec2;
    vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(100.0, 0.0)),
            // Arrives straight, leaves on a curve: easy for an insert to get wrong.
            PenAnchor {
                at: Point::new(100.0, 100.0),
                out: Vec2::new(-40.0, 20.0),
                back: Vec2::ZERO,
                radius: 0.0,
            },
            PenAnchor {
                at: Point::new(0.0, 100.0),
                out: Vec2::ZERO,
                back: Vec2::new(40.0, 20.0),
                radius: 0.0,
            },
        ],
        closed: true,
    }]
}

/// Asserts two paths trace the same shape however each is cut into segments: every
/// sample of each lies on the other, within the nearest-point solve's accuracy.
fn assert_same_shape(a: &BezPath, b: &BezPath) {
    for (from, onto, which) in [(a, b, "a→b"), (b, a, "b→a")] {
        for (i, seg) in from.segments().enumerate() {
            for f in 0..=8 {
                let p = seg.eval(f as f64 / 8.0);
                let hit =
                    crate::geometry::nearest_segment(onto, p).expect("the other path has segments");
                assert!(
                    hit.distance < 1e-3,
                    "{which}: segment {i} at t={} is {:.6} off the other path",
                    f as f64 / 8.0,
                    hit.distance
                );
            }
        }
    }
}

/// Inserting a point leaves the shape unchanged (sampled, since one cubic becomes two)
/// and survives the `pen_anchors` round trip.
#[test]
fn inserting_a_point_splits_a_segment_without_moving_the_curve() {
    for (at, t) in [((0, 2), 0.5), ((0, 2), 0.25), ((0, 0), 0.5), ((0, 3), 0.5)] {
        let before = quad_with_a_curve();
        let mut after = before.clone();
        let new = insert_point(&mut after, at, t).expect("a segment to split");
        assert_eq!(new, (at.0, at.1 + 1), "the new anchor follows the old one");
        assert_eq!(after[0].anchors.len(), before[0].anchors.len() + 1);
        assert!(after[0].closed, "closure survives an insert");

        let (a, b) = (pen_path(&before), pen_path(&after));
        assert_same_shape(&a, &b);
        assert_same_path(&b, &pen_path(&pen_anchors(&b, &[])));
    }
}

/// Splitting a straight segment writes two lines and a corner, not two flat cubics.
#[test]
fn splitting_a_line_leaves_two_lines_and_a_corner() {
    let mut subs = quad_with_a_curve();
    insert_point(&mut subs, (0, 0), 0.5).unwrap();
    assert!(subs[0].anchors[1].is_corner(), "the new point is a corner");
    assert_eq!(subs[0].anchors[1].at, Point::new(50.0, 0.0));
    let els = pen_path(&subs).elements().to_vec();
    assert!(
        matches!(els[1], PathEl::LineTo(_)) && matches!(els[2], PathEl::LineTo(_)),
        "{els:#?}"
    );
}

/// A radius rides on its anchor, so an insert cannot slide one onto the neighbouring corner.
#[test]
fn an_insert_leaves_every_radius_on_the_corner_that_owned_it() {
    let mut subs = quad_with_a_curve();
    subs[0].anchors[0].radius = 3.0;
    subs[0].anchors[1].radius = 8.0;
    insert_point(&mut subs, (0, 0), 0.5).unwrap();
    let radii: Vec<f64> = subs[0].anchors.iter().map(|a| a.radius).collect();
    assert_eq!(
        radii,
        vec![3.0, 0.0, 8.0, 0.0, 0.0],
        "the new point is at 1"
    );
    assert_eq!(pen_radii(&subs), vec![3.0, 0.0, 8.0]);
}

/// Removing a segment opens a closed run starting after the cut, including the closing segment.
#[test]
fn removing_a_segment_opens_a_closed_run_at_the_cut() {
    use crate::kurbo::Vec2;
    let subs = quad_with_a_curve();
    let corners: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();

    let left = remove_segments(&subs, &[(0, 1)]);
    assert_eq!(left.len(), 1);
    assert!(!left[0].closed, "a cut run cannot still be closed");
    let got: Vec<Point> = left[0].anchors.iter().map(|a| a.at).collect();
    assert_eq!(got, vec![corners[2], corners[3], corners[0], corners[1]]);
    // The handles of the removed segment are cleared, or they would reappear on closing.
    assert_eq!(left[0].anchors[0].back, Vec2::ZERO);
    assert_eq!(left[0].anchors[3].out, Vec2::ZERO);

    // The closing segment: the same order, merely open.
    let closing = remove_segments(&subs, &[(0, 3)]);
    let got: Vec<Point> = closing[0].anchors.iter().map(|a| a.at).collect();
    assert_eq!(got, corners);
    assert!(!closing[0].closed);
}

/// On an open run removal splits it, and a remainder too short to draw is dropped.
#[test]
fn removing_a_segment_splits_an_open_run_and_drops_a_lone_point() {
    let mut subs = quad_with_a_curve();
    subs[0].closed = false;

    let two = remove_segments(&subs, &[(0, 1)]);
    assert_eq!(two.len(), 2, "one run became two");
    assert_eq!(two[0].anchors.len(), 2);
    assert_eq!(two[1].anchors.len(), 2);

    let one = remove_segments(&subs, &[(0, 0)]);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].anchors.len(), 3);

    // The last anchor of an open run has no segment leaving it: nothing is removed.
    assert!(segment_ends(&subs, (0, 3)).is_none());
    let none = remove_segments(&subs, &[(0, 3)]);
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].anchors.len(), 4);

    let pair = vec![PenSubpath {
        anchors: subs[0].anchors[..2].to_vec(),
        closed: false,
    }];
    assert!(remove_segments(&pair, &[(0, 0)]).is_empty());
}

/// Splitting a closed run at an anchor opens it there with both ends on that
/// anchor, each keeping the handle on its side, and draws the same outline.
/// flip: `split_at_points` zeroing neither handle → fails at "the closing end
/// keeps no leaving handle".
#[test]
fn splitting_a_closed_run_opens_it_at_the_anchor() {
    use crate::kurbo::Vec2;
    let subs = quad_with_a_curve();
    let (split, pairs) = split_at_points(&subs, &[(0, 2)]);
    assert_eq!(split.len(), 1);
    assert!(!split[0].closed, "opened");
    let got: Vec<Point> = split[0].anchors.iter().map(|a| a.at).collect();
    let at = subs[0].anchors[2].at;
    assert_eq!(got.first(), Some(&at));
    assert_eq!(got.last(), Some(&at), "both ends on the split anchor");
    assert_eq!(got.len(), 5);
    assert_eq!(pairs, vec![((0, 4), (0, 0))]);
    assert_eq!(split[0].anchors[0].out, Vec2::new(-40.0, 20.0));
    assert_eq!(
        split[0].anchors[4].out,
        Vec2::ZERO,
        "the closing end keeps no leaving handle"
    );
    let whole = pen_outline(&subs[0].anchors, true);
    let opened = pen_outline(&split[0].anchors, false);
    assert_same_shape(&whole, &opened);
}

/// *Add points* cuts every segment, the curve too, into pieces that draw the
/// same outline; and *Remove redundant points* takes exactly those anchors back
/// out, leaving the four it started from.
///
/// flip: `merged_handles` without its cubic case → fails at the count taken
/// out, 6 of 8 (only the straight sides' points go).
#[test]
fn added_points_draw_the_same_and_are_redundant() {
    let subs = quad_with_a_curve();
    let whole = pen_outline(&subs[0].anchors, true);
    let three = split_segments(&subs, 3);
    assert_eq!(three[0].anchors.len(), 12);
    assert!(three[0].closed);
    assert_same_shape(&whole, &pen_outline(&three[0].anchors, true));

    let (back, gone) = redundant_points(&three);
    assert_eq!(gone.len(), 8);
    assert_eq!(back[0].anchors.len(), 4, "back to four");
    assert_same_shape(&whole, &pen_outline(&back[0].anchors, true));
    let (none, nothing) = redundant_points(&subs);
    assert!(nothing.is_empty(), "every corner of the quad shapes it");
    assert_eq!(none[0].anchors.len(), 4);
}

/// Next to a rounded corner, whose fillet is clamped to half of each leg, neither
/// *Remove redundant points* nor *Add points* changes a leg, so the rounded outline
/// draws as before.
/// flip: `merged_handles` refusing only a rounded `b` → fails at "the point beside
/// the fillet stays"; `split_segments` cutting a rounded corner's legs → fails at
/// "the legs of a rounded corner stay whole".
#[test]
fn points_beside_a_rounded_corner_are_neither_removed_nor_added() {
    let drawn =
        |subs: &[PenSubpath]| crate::geometry::round_corners(&pen_path(subs), &pen_radii(subs));
    let corner = |x, y, radius| PenAnchor {
        radius,
        ..PenAnchor::corner(Point::new(x, y))
    };
    let shelf = vec![PenSubpath {
        anchors: vec![
            corner(0.0, 0.0, 40.0),
            corner(20.0, 0.0, 0.0),
            corner(100.0, 0.0, 0.0),
            corner(100.0, 100.0, 0.0),
            corner(0.0, 100.0, 0.0),
        ],
        closed: true,
    }];
    let (left, gone) = redundant_points(&shelf);
    assert!(
        gone.is_empty(),
        "the point beside the fillet stays: {gone:?}"
    );
    assert_same_shape(&drawn(&shelf), &drawn(&left));

    let mut square = shelf.clone();
    square[0].anchors.remove(1);
    let halves = split_segments(&square, 2);
    assert_eq!(
        halves[0].anchors.len(),
        6,
        "the legs of a rounded corner stay whole"
    );
    assert_same_shape(&drawn(&square), &drawn(&halves));
    let (back, _) = redundant_points(&halves);
    assert_eq!(back, square, "the two still undo each other");
}

/// A point twinned on the one before it is redundant, and so is one in the
/// middle of a straight run; an open run's ends never are.
#[test]
fn twins_and_straight_middles_are_redundant() {
    let corner = |x, y| PenAnchor::corner(Point::new(x, y));
    let run = vec![PenSubpath {
        anchors: vec![
            corner(0.0, 0.0),
            corner(0.0, 0.0),
            corner(50.0, 0.0),
            corner(100.0, 0.0),
            corner(100.0, 50.0),
        ],
        closed: false,
    }];
    let (left, gone) = redundant_points(&run);
    let at: Vec<Point> = left[0].anchors.iter().map(|a| a.at).collect();
    assert_eq!(
        at,
        vec![
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
            Point::new(100.0, 50.0)
        ]
    );
    assert_eq!(gone, vec![Point::new(0.0, 0.0), Point::new(50.0, 0.0)]);
}

/// A closed run whose first anchor is the middle of a straight side, as an
/// import starting mid-edge has, loses that anchor at the seam.
/// flip: `redundant_points` without its seam's first-anchor arm → fails at "the
/// first anchor goes".
#[test]
fn a_redundant_first_anchor_goes_at_the_seam() {
    let corner = |x, y| PenAnchor::corner(Point::new(x, y));
    let square = vec![PenSubpath {
        anchors: vec![
            corner(50.0, 0.0),
            corner(100.0, 0.0),
            corner(100.0, 100.0),
            corner(0.0, 100.0),
            corner(0.0, 0.0),
        ],
        closed: true,
    }];
    let (left, gone) = redundant_points(&square);
    assert_eq!(gone, vec![Point::new(50.0, 0.0)], "the first anchor goes");
    let at: Vec<Point> = left[0].anchors.iter().map(|a| a.at).collect();
    assert_eq!(
        at,
        [(100.0, 0.0), (100.0, 100.0), (0.0, 100.0), (0.0, 0.0)].map(|(x, y)| Point::new(x, y))
    );
    assert_same_shape(
        &pen_outline(&square[0].anchors, true),
        &pen_outline(&left[0].anchors, true),
    );
}

/// *Smooth selected* moves a corner's handles the amount of the way to fresh
/// smooth ones, and leaves the anchors it was not given.
#[test]
fn smoothing_by_an_amount_moves_handles_part_way() {
    let mut subs = quad_with_a_curve();
    let mut full = subs.clone();
    smooth_points(&mut full, &[(0, 1)], 1.0);
    smooth_points(&mut subs, &[(0, 1)], 0.5);
    let (half, whole) = (subs[0].anchors[1], full[0].anchors[1]);
    assert!(whole.out.hypot() > 1.0, "fully smooth has handles");
    assert_eq!(half.out, whole.out * 0.5);
    assert_eq!(half.back, whole.back * 0.5);
    assert_eq!(subs[0].anchors[0].out, Vec2::ZERO, "only the anchor given");
}

/// A rounded corner is not split, since an open end cannot carry its fillet and
/// the outline would lose its rounding; a radius on a smooth anchor, which draws
/// nothing, does not stop a split.
/// flip: `splittable` without its `rounded` test → fails at "a rounded corner refuses".
#[test]
fn a_rounded_corner_is_not_split() {
    let mut subs = quad_with_a_curve();
    subs[0].anchors[1].radius = 10.0;
    assert!(!splittable(&subs, (0, 1)), "a rounded corner refuses");
    assert_eq!(
        split_refusal(&subs, &[(0, 1)]),
        Some("A rounded corner can't be split")
    );
    let (unchanged, none) = split_at_points(&subs, &[(0, 1)]);
    assert_eq!(unchanged, subs);
    assert!(none.is_empty());
    assert_eq!(
        split_refusal(&subs, &[(0, 1), (0, 0)]),
        None,
        "another point cuts"
    );
    let mut smooth = subs.clone();
    smooth[0].anchors[1].out = Vec2::new(10.0, 0.0);
    smooth[0].anchors[1].back = Vec2::new(-10.0, 0.0);
    assert!(splittable(&smooth, (0, 1)));
}

/// An open run splits in two at an inner anchor, and its ends refuse.
#[test]
fn splitting_an_open_run_makes_two_and_its_ends_refuse() {
    let mut subs = quad_with_a_curve();
    subs[0].closed = false;
    assert!(!splittable(&subs, (0, 0)) && !splittable(&subs, (0, 3)));
    let (unchanged, none) = split_at_points(&subs, &[(0, 0), (0, 3)]);
    assert_eq!(unchanged.len(), 1, "an open end has nothing to split");
    assert_eq!(unchanged[0].anchors.len(), 4);
    assert!(none.is_empty());

    let (two, pairs) = split_at_points(&subs, &[(0, 1), (0, 2)]);
    let lens: Vec<usize> = two.iter().map(|s| s.anchors.len()).collect();
    assert_eq!(lens, vec![2, 2, 2], "three runs from two cuts");
    assert_eq!(pairs, vec![((0, 1), (1, 0)), ((1, 1), (2, 0))]);
    assert_eq!(two[1].anchors[0].at, subs[0].anchors[1].at);
}

/// A segment drag is `move_points` over the ends `segment_ends` names: both anchors
/// move, handles ride along, nothing else does.
#[test]
fn moving_a_segment_carries_both_its_anchors_and_no_others() {
    use crate::kurbo::Vec2;
    let mut subs = quad_with_a_curve();
    let before = subs.clone();
    let delta = Vec2::new(10.0, -5.0);
    let (a, b) = segment_ends(&subs, (0, 2)).expect("a segment");
    move_points(&mut subs, &[a, b], delta);
    for i in 0..4 {
        let (a, b) = (before[0].anchors[i], subs[0].anchors[i]);
        let want = if i == 2 || i == 3 { a.at + delta } else { a.at };
        assert_eq!(b.at, want, "anchor {i}");
        assert_eq!(b.out, a.out, "anchor {i}: handles ride along unchanged");
        assert_eq!(b.back, a.back, "anchor {i}");
    }
}

/// Two cuts in a closed quad leave two open runs; the anchor order is asserted, since
/// the closed-to-open rotation is where an off-by-one would live.
#[test]
fn removing_several_segments_at_once_leaves_a_run_between_each_pair() {
    let subs = quad_with_a_curve();
    let p: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();

    let two = remove_segments(&subs, &[(0, 0), (0, 2)]);
    assert_eq!(two.len(), 2);
    let got: Vec<Vec<Point>> = two
        .iter()
        .map(|s| s.anchors.iter().map(|a| a.at).collect())
        .collect();
    assert_eq!(got, vec![vec![p[1], p[2]], vec![p[3], p[0]]], "{got:?}");
    assert!(two.iter().all(|s| !s.closed));

    assert!(remove_segments(&subs, &[(0, 0), (0, 1), (0, 2), (0, 3)]).is_empty());

    // An index naming no segment is ignored (an open run's last anchor has none).
    let mut open = subs.clone();
    open[0].closed = false;
    let one = remove_segments(&open, &[(0, 3), (0, 1)]);
    let sizes: Vec<usize> = one.iter().map(|s| s.anchors.len()).collect();
    assert_eq!(sizes, vec![2, 2], "only the real cut took");
}

/// A bend puts the grabbed point exactly under the pointer at any `t` and moves no anchor.
#[test]
fn bending_a_segment_moves_the_grabbed_point_by_the_whole_delta() {
    use crate::kurbo::Vec2;
    for t in [0.25, 0.5, 0.75] {
        let mut subs = quad_with_a_curve();
        let before = segment_point(&subs, (0, 0), t).unwrap();
        let ends = (subs[0].anchors[0].at, subs[0].anchors[1].at);
        let delta = Vec2::new(6.0, -30.0);
        bend_segment(&mut subs, (0, 0), t, delta);

        let after = segment_point(&subs, (0, 0), t).unwrap();
        assert!(
            (after - (before + delta)).hypot() < 1e-9,
            "t={t}: {after:?} vs {:?}",
            before + delta
        );
        assert_eq!(
            (subs[0].anchors[0].at, subs[0].anchors[1].at),
            ends,
            "t={t}: a bend moves no anchor"
        );
        assert!(!subs[0].anchors[0].is_corner(), "t={t}: it curves now");
        // The other side of each anchor is untouched.
        assert_eq!(subs[0].anchors[0].back, Vec2::ZERO);
        assert_eq!(subs[0].anchors[1].out, Vec2::ZERO);
    }

    // At the ends the control points have no leverage, so the bend declines.
    let mut subs = quad_with_a_curve();
    let before = subs[0].anchors[0].out;
    bend_segment(&mut subs, (0, 0), 0.0, Vec2::new(10.0, 10.0));
    assert_eq!(subs[0].anchors[0].out, before);
}

/// The point box is the selected anchors' extent; a handle drag scales the set about
/// the opposite corner under `resize_factors`' rule.
#[test]
fn a_point_box_scales_the_selected_points_about_the_held_corner() {
    let subs = quad_with_a_curve();
    let all: Vec<PointRef> = (0..4).map(|i| (0, i)).collect();
    let box_ = points_box(&subs, &all).expect("four points make a box");
    assert_eq!(box_, Rect::new(0.0, 0.0, 100.0, 100.0));
    // One point or a flat pair is no box: its handles would be unreachable.
    assert!(points_box(&subs, &[(0, 0)]).is_none());
    assert!(points_box(&subs, &[(0, 0), (0, 1)]).is_none());

    let mut scaled = subs.clone();
    resize_points(
        &mut scaled,
        &all,
        box_,
        Handle::BottomRight,
        Point::new(200.0, 200.0),
        Resize::geometry(false, false),
    );
    assert_eq!(scaled[0].anchors[0].at, Point::new(0.0, 0.0));
    assert_eq!(scaled[0].anchors[2].at, Point::new(200.0, 200.0));
    assert_eq!(scaled[0].anchors[2].out, subs[0].anchors[2].out * 2.0);

    // Only the selected points move.
    let picked = [(0, 0), (0, 2)];
    let mut some = subs.clone();
    resize_points(
        &mut some,
        &picked,
        points_box(&subs, &picked).unwrap(),
        Handle::BottomRight,
        Point::new(200.0, 200.0),
        Resize::geometry(false, false),
    );
    assert_eq!(some[0].anchors[2].at, Point::new(200.0, 200.0));
    assert_eq!(some[0].anchors[1].at, subs[0].anchors[1].at, "unselected");
    assert_eq!(some[0].anchors[3].at, subs[0].anchors[3].at, "unselected");
}

/// Every handle dropped where it was picked up scales by one; a min-side handle starts
/// at −extent from its anchor, so the identity is asked of all eight.
#[test]
fn a_handle_dropped_where_it_was_grabbed_scales_by_one() {
    let box_ = Rect::new(10.0, 20.0, 110.0, 70.0);
    for handle in Handle::CORNERS.into_iter().chain(Handle::EDGES) {
        let (ux, uy) = handle.unit();
        let at = Point::new(
            box_.min_x() + box_.width() * ux,
            box_.min_y() + box_.height() * uy,
        );
        for symmetric in [false, true] {
            let (sx, sy, _) = resize_factors(box_, handle, at, false, symmetric);
            assert!(
                (sx - 1.0).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9,
                "{handle:?} symmetric={symmetric} at rest gave ({sx}, {sy})"
            );
        }
    }
}

/// A dragged min-side handle follows the pointer and holds the opposite side.
#[test]
fn a_min_side_handle_scales_rather_than_mirroring() {
    let box_ = Rect::new(0.0, 0.0, 100.0, 50.0);
    let (sx, sy, anchor) = resize_factors(
        box_,
        Handle::TopLeft,
        Point::new(-100.0, -50.0),
        false,
        false,
    );
    assert!(
        (sx - 2.0).abs() < 1e-9 && (sy - 2.0).abs() < 1e-9,
        "{sx}, {sy}"
    );
    assert_eq!(anchor, Point::new(100.0, 50.0));

    let (sx, sy, anchor) = resize_factors(box_, Handle::Left, Point::new(50.0, 25.0), false, false);
    assert!(
        (sx - 0.5).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9,
        "{sx}, {sy}"
    );
    assert_eq!(anchor.x, 100.0);

    // Dragged past the anchor it holds, the sign flips.
    let (sx, _, _) = resize_factors(box_, Handle::TopLeft, Point::new(200.0, 0.0), false, false);
    assert!(sx < 0.0, "a handle dragged past its anchor mirrors: {sx}");
}

/// `geometry::nearest_segment`'s `t` and `segment_cubic`'s agree on a line as on a
/// curve, since a straight segment is degree-elevated to keep the linear parameter.
#[test]
fn a_segments_parameter_means_the_same_to_the_hit_test_and_to_the_app() {
    use crate::kurbo::Vec2;
    let subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(300.0, 0.0)),
            PenAnchor::smooth(Point::new(400.0, 200.0), Vec2::new(0.0, 90.0)),
            PenAnchor::corner(Point::new(100.0, 260.0)),
        ],
        closed: false,
    }];
    let path = pen_path(&subs);
    for seg in 0..3usize {
        let cubic = segment_cubic(&subs, (0, seg)).expect("a segment");
        for k in 1..8 {
            let u = k as f64 / 8.0;
            let pointer = cubic.eval(u) + Vec2::new(0.0, -2.0);
            let hit = crate::geometry::nearest_segment(&path, pointer).expect("a hit");
            let mine = segment_point(&subs, (hit.subpath, hit.anchor), hit.t).expect("a point");
            assert!(
                (mine - hit.at).hypot() < 1e-6,
                "segment {seg} at u={u}: core says {:?}, the app says {mine:?}",
                hit.at
            );
        }
    }

    // The straight one is exactly a lerp.
    let line = segment_cubic(&subs, (0, 0)).unwrap();
    for k in 0..=4 {
        let t = k as f64 / 4.0;
        assert!((line.eval(t).x - 300.0 * t).abs() < 1e-9, "t={t}");
    }
}

/// A bend on a straight segment grabs the point the pointer took, and a zero bend writes nothing.
#[test]
fn bending_a_straight_segment_moves_the_point_the_pointer_took() {
    use crate::kurbo::Vec2;
    for t in [0.25, 0.5, 0.75] {
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(300.0, 0.0)),
            ],
            closed: false,
        }];
        let before = segment_point(&subs, (0, 0), t).unwrap();
        assert!((before.x - 300.0 * t).abs() < 1e-9, "the grab is a lerp");
        let delta = Vec2::new(10.0, -40.0);
        bend_segment(&mut subs, (0, 0), t, delta);
        let after = segment_point(&subs, (0, 0), t).unwrap();
        assert!(
            (after - (before + delta)).hypot() < 1e-9,
            "t={t}: {after:?} vs {:?}",
            before + delta
        );
    }

    // A Ctrl-press that never travelled must not elevate the line.
    let mut subs = vec![PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(300.0, 0.0)),
        ],
        closed: false,
    }];
    bend_segment(&mut subs, (0, 0), 0.5, Vec2::ZERO);
    assert!(subs[0].anchors[0].is_corner(), "still a plain line");
}

/// A dragged handle snaps to the direction that makes its anchor smooth, keeping its
/// length; the tolerance is a perpendicular distance, not an angle.
#[test]
fn a_dragged_handle_snaps_to_the_direction_that_makes_its_anchor_smooth() {
    use crate::kurbo::Vec2;
    let at = Point::new(100.0, 100.0);
    // The smooth direction for the outgoing handle is the opposite of the incoming one.
    let broken = PenAnchor {
        at,
        out: Vec2::new(5.0, 0.0),
        back: Vec2::new(-30.0, -40.0),
        radius: 0.0,
    };
    let smooth_dir = Vec2::new(30.0, 40.0).normalize();

    let want = at + smooth_dir * 50.0;
    let perp = Vec2::new(-smooth_dir.y, smooth_dir.x) * 3.0;
    let got = tangent_snap(&broken, Side::Out, want + perp, 4.0);
    assert!((got - want).hypot() < 1e-9, "{got:?} vs {want:?}");

    // 6 units off is past the tolerance, and free aim wins.
    let free = want + perp * 2.0;
    assert_eq!(tangent_snap(&broken, Side::Out, free, 4.0), free);

    // The same offset at four times the length still snaps; an angular tolerance would not.
    let far = at + smooth_dir * 200.0;
    let got = tangent_snap(&broken, Side::Out, far + perp, 4.0);
    assert!((got - far).hypot() < 1e-9, "{got:?} vs {far:?}");

    // The eight compass directions are candidates too.
    let axis = at + Vec2::new(60.0, 0.0);
    let got = tangent_snap(&broken, Side::Back, axis + Vec2::new(0.0, 2.0), 4.0);
    assert!((got - axis).hypot() < 1e-9, "{got:?} vs {axis:?}");

    // A candidate behind the pointer is not offered: it would flip the handle through its anchor.
    let behind = at - smooth_dir * 50.0;
    let got = tangent_snap(&broken, Side::Out, behind, 4.0);
    assert!(
        (got - behind).hypot() < 1e-9 || (got - at).dot(smooth_dir) < 0.0,
        "the handle jumped to the other side: {got:?}"
    );

    // A corner offers no smooth direction, only the compass points.
    let corner = PenAnchor::corner(at);
    let off = at + Vec2::new(50.0, 20.0);
    assert_eq!(tangent_snap(&corner, Side::Out, off, 4.0), off);

    // Nor does an already-smooth anchor: its own direction would make a slow drag stick, then jump.
    let mirrored = PenAnchor::smooth(at, Vec2::new(40.0, 10.0));
    let nudged = at + Vec2::new(41.0, 11.5);
    assert_eq!(
        tangent_snap(&mirrored, Side::Out, nudged, 4.0),
        nudged,
        "a smooth anchor snapped its own handle back to where it was"
    );

    // Through `pull_handle`, which the drag calls; Alt so the mirror does not realign it.
    let mut subs = vec![PenSubpath {
        anchors: vec![broken],
        closed: false,
    }];
    pull_handle(&mut subs, (0, 0), Side::Out, want + perp, true, 0.0, 4.0);
    assert!(
        (subs[0].anchors[0].leaving() - want).hypot() < 1e-9,
        "the drag did not ask for the tangent snap: {:?}",
        subs[0].anchors[0].leaving()
    );
}

/// A run of `n` anchors along the x axis, offset so two runs are tellable apart.
fn run(n: usize, x0: f64, closed: bool) -> PenSubpath {
    PenSubpath {
        anchors: (0..n)
            .map(|i| PenAnchor::corner(Point::new(x0 + i as f64 * 10.0, 0.0)))
            .collect(),
        closed,
    }
}

/// Join takes two open ends and refuses the rest, including both ends of a two-anchor
/// run, whose closing segment would retrace its only one (`context-menus.md` §6.5).
///
/// flip: drop the `anchors.len() < 3` arm → the last case comes back `Some`; check `sub.closed` on only the first ref → the second-ref closed case fails
#[test]
fn join_takes_two_open_ends_and_refuses_everything_else() {
    let two_runs = [run(3, 0.0, false), run(3, 100.0, false)];
    let ok = |points: &[PointRef]| joinable_ends(&two_runs, points).is_some();

    assert!(ok(&[(0, 2), (1, 0)]));
    assert!(ok(&[(0, 0), (1, 2)]), "either end of either run");
    assert!(ok(&[(0, 0), (0, 2)]));

    // A middle anchor, the wrong count, nothing, and a missing index all refuse.
    assert!(!ok(&[(0, 1), (1, 0)]));
    assert!(!ok(&[(0, 0)]));
    assert!(!ok(&[(0, 0), (0, 2), (1, 0)]));
    assert!(!ok(&[]));
    assert!(!ok(&[(0, 9), (1, 0)]));

    // A closed run has no ends, in either position of the pair.
    let closed = [run(3, 0.0, true), run(3, 100.0, false)];
    assert!(!joinable_ends(&closed, &[(0, 0), (1, 0)]).is_some());
    assert!(!joinable_ends(&closed, &[(1, 0), (0, 0)]).is_some());

    let short = [run(2, 0.0, false), run(3, 100.0, false)];
    assert!(
        joinable_ends(&short, &[(0, 0), (0, 1)]).is_none(),
        "closing a two-anchor run retraces its only segment"
    );
    assert!(
        joinable_ends(&short, &[(0, 0), (1, 0)]).is_some(),
        "but splicing that same run onto another one is fine"
    );
}

/// A splice runs from one far end to the other through the seam for all four end
/// combinations; asserted on the position sequence, since a wrong turn keeps the length.
///
/// flip: reverse the head on `a.1 == last` instead of `a.1 == 0` → the two `(0, 0)` cases fail
#[test]
fn a_splice_reads_end_to_end_through_the_seam() {
    let subpaths = [run(3, 0.0, false), run(3, 100.0, false)];
    let xs = |sub: &PenSubpath| sub.anchors.iter().map(|a| a.at.x).collect::<Vec<_>>();

    for (a, b, want) in [
        // grabbed at 0/20 → the head turns; grabbed at 100 → the tail does not.
        ((0, 2), (1, 0), vec![0.0, 10.0, 20.0, 100.0, 110.0, 120.0]),
        ((0, 2), (1, 2), vec![0.0, 10.0, 20.0, 120.0, 110.0, 100.0]),
        ((0, 0), (1, 0), vec![20.0, 10.0, 0.0, 100.0, 110.0, 120.0]),
        ((0, 0), (1, 2), vec![20.0, 10.0, 0.0, 120.0, 110.0, 100.0]),
    ] {
        let (list, seam) = join_ends(&subpaths, a, b).expect("these are open ends");
        assert_eq!(list.len(), 1, "{a:?}→{b:?}: the two runs became one");
        assert_eq!(xs(&list[0]), want, "{a:?}→{b:?}");
        assert!(!list[0].closed, "a splice leaves the run open");
        assert_eq!(seam, (0, 2), "{a:?}→{b:?}");
    }
}

/// The joined run keeps the lower slot, other subpaths are not renumbered, and the seam
/// names the surviving slot.
///
/// flip: write the joined run to `a.0` and remove `b.0` → fails with the pair reversed
#[test]
fn a_splice_keeps_the_lower_slot_whichever_end_was_grabbed_first() {
    let untouched = run(4, 500.0, true);
    let subpaths = [run(3, 0.0, false), untouched.clone(), run(3, 100.0, false)];
    for (a, b) in [((2, 0), (0, 2)), ((0, 2), (2, 0))] {
        let (list, seam) = join_ends(&subpaths, a, b).expect("these are open ends");
        assert_eq!(list.len(), 2, "{a:?}→{b:?}");
        assert_eq!(list[0].anchors.len(), 6, "the join is in slot 0");
        assert_eq!(
            list[1].anchors.iter().map(|p| p.at.x).collect::<Vec<_>>(),
            untouched.anchors.iter().map(|p| p.at.x).collect::<Vec<_>>(),
            "{a:?}→{b:?}: the run nobody touched came through unchanged"
        );
        assert_eq!(seam.0, 0, "{a:?}→{b:?}: the seam names the surviving slot");
    }
}

/// Both ends of one run close it, and the closing segment is named by the run's last anchor.
///
/// flip: return `(a.0, 0)` for the seam → fails at the seam assertion
#[test]
fn closing_a_run_adds_no_anchors_and_names_the_closing_segment() {
    let subpaths = [run(4, 0.0, false), run(3, 100.0, false)];
    let (list, seam) = join_ends(&subpaths, (0, 0), (0, 3)).expect("its own two ends");
    assert_eq!(list.len(), 2, "closing removes no subpath");
    assert_eq!(list[0].anchors.len(), 4, "and adds no anchor");
    assert!(list[0].closed);
    assert!(!list[1].closed, "the other run is left alone");
    assert_eq!(seam, (0, 3));
}

/// Reversing a run flips its winding (the signed area's sign) and nothing else: the
/// magnitude survives, the radius rides along, and unnamed runs are left alone.
#[test]
fn reversing_a_subpath_flips_its_winding_and_leaves_the_others_alone() {
    use crate::kurbo::Shape;

    let ring = |x: f64, y: f64, w: f64| PenSubpath {
        anchors: vec![
            PenAnchor::corner(Point::new(x, y)),
            PenAnchor::corner(Point::new(x + w, y)),
            PenAnchor::corner(Point::new(x + w, y + w)),
            PenAnchor::corner(Point::new(x, y + w)),
        ],
        closed: true,
    };
    let mut subs = vec![ring(0.0, 0.0, 100.0), ring(25.0, 25.0, 50.0)];
    subs[1].anchors[2].radius = 6.0;
    let area = |s: &PenSubpath| pen_outline(&s.anchors, s.closed).area();
    let (outer0, inner0) = (area(&subs[0]), area(&subs[1]));
    assert!(
        outer0.signum() == inner0.signum(),
        "the fixture must start with both runs wound the same way"
    );

    reverse_subpaths(&mut subs, &[1]);
    let (outer1, inner1) = (area(&subs[0]), area(&subs[1]));
    assert_eq!(
        inner1.signum(),
        -inner0.signum(),
        "the inner run still winds {inner0} — non-zero fills it solid instead of \
         cutting a hole"
    );
    assert!(
        (inner1.abs() - inner0.abs()).abs() < 1e-9,
        "the reversal changed the shape: |area| {} → {}",
        inner0.abs(),
        inner1.abs()
    );
    assert_eq!(outer1, outer0, "the run nobody named was turned round too");
    assert_eq!(subs[1].anchors[1].radius, 6.0, "the radius left its corner");
    assert_eq!(pen_radii(&subs), vec![0.0, 0.0, 0.0, 0.0, 0.0, 6.0]);
}

/// `subpaths_touched` lists the runs a point selection names, ascending and deduplicated.
#[test]
fn the_subpaths_a_selection_touches_are_listed_once_each() {
    assert_eq!(
        subpaths_touched(&[(2, 0), (0, 5), (2, 1), (0, 0), (1, 3)]),
        vec![0, 1, 2]
    );
    assert!(subpaths_touched(&[]).is_empty());
}
// --- placing an image (§5.5a) --------------------------------------------

fn entry_of(w: u32, h: u32) -> ImageEntry {
    ImageEntry {
        source: crate::ImageSource::Embedded(vec![1, 2, 3].into()),
        format: crate::ImageFormat::Png,
        width: w,
        height: h,
    }
}

/// Fit shrinks an oversized image and never scales a small one up.
#[test]
fn a_placed_image_shrinks_to_fit_and_is_never_scaled_up() {
    let frame = Size::new(1000.0, 800.0);

    assert_eq!(
        fitted_image_size(Size::new(32.0, 32.0), frame),
        Size::new(32.0, 32.0)
    );
    assert_eq!(fitted_image_size(frame, frame), frame);

    let wide = fitted_image_size(Size::new(4000.0, 2000.0), frame);
    assert_eq!(wide, Size::new(1000.0, 500.0));

    let tall = fitted_image_size(Size::new(1000.0, 4000.0), frame);
    assert_eq!(tall, Size::new(200.0, 800.0));
    assert!(
        tall.height <= frame.height && tall.width <= frame.width,
        "a fitted image must be inside the box it was fitted to, got {tall:?}"
    );
}

/// A frame with no size falls back to the image's intrinsic size.
#[test]
fn a_degenerate_frame_places_the_image_at_its_own_size() {
    let intrinsic = Size::new(64.0, 48.0);
    assert_eq!(fitted_image_size(intrinsic, Size::ZERO), intrinsic);
    assert_eq!(
        fitted_image_size(intrinsic, Size::new(-10.0, 100.0)),
        intrinsic
    );
}

/// The click point is the placed image's centre.
#[test]
fn a_placed_image_is_centred_on_the_click() {
    let mut ids = IdSource::new(1);
    let (node, parent) = (ids.mint(), ids.mint());
    let tx = create_image(
        node,
        parent,
        0,
        Affine::IDENTITY,
        Point::new(500.0, 400.0),
        Size::new(1000.0, 1000.0),
        ImageId("sha256:x".into()),
        Some(entry_of(200, 100)),
        Size::new(200.0, 100.0),
        None,
    );
    let Operation::CreateNode {
        kind, transform, ..
    } =
        tx.0.iter()
            .find(|o| matches!(o, Operation::CreateNode { .. }))
            .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        *kind,
        NodeKind::Rect {
            size: Size::new(200.0, 100.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        "it fits, so it keeps its pixel size"
    );
    assert_eq!(
        transform.unwrap().translation(),
        Vec2::new(400.0, 350.0),
        "the click should be the centre of the placed box"
    );
}

/// The `NodeKind` a creation builder writes, for the tests below.
fn made(tx: &Transaction) -> NodeKind {
    match tx
        .0
        .iter()
        .find(|o| matches!(o, Operation::CreateNode { .. }))
    {
        Some(Operation::CreateNode { kind, .. }) => kind.clone(),
        _ => panic!("a creation builder makes a node"),
    }
}

/// `create_polygon` and `create_star` clamp their count at the commit door as
/// `step_sides` does at the keyboard door. Both are driven: different fields.
///
/// flip: delete either `clamp` → fails at that builder's out-of-range case
#[test]
fn the_creation_builders_clamp_their_side_count_at_both_ends() {
    let mut ids = IdSource::new(1);
    let (parent, a, b) = (ids.mint(), Point::ZERO, Point::new(100.0, 100.0));
    let mut poly = |sides| {
        create_polygon(
            ids.mint(),
            parent,
            0,
            Affine::IDENTITY,
            a,
            b,
            sides,
            Color::BLACK,
        )
    };
    for (asked, want) in [
        (0, 3),
        (2, 3),
        (3, 3),
        (7, 7),
        (64, 64),
        (65, 64),
        (u32::MAX, 64),
    ] {
        match made(&poly(asked)) {
            NodeKind::Polygon { sides, .. } => assert_eq!(
                sides, want,
                "a polygon asked for {asked} sides must be made with {want}"
            ),
            other => panic!("create_polygon makes a polygon, not {other:?}"),
        }
    }

    let mut star = |points| {
        create_star(
            ids.mint(),
            parent,
            0,
            Affine::IDENTITY,
            a,
            b,
            points,
            Color::BLACK,
        )
    };
    for (asked, want) in [(0, 3), (2, 3), (5, 5), (65, 64), (u32::MAX, 64)] {
        match made(&star(asked)) {
            NodeKind::Star { points, .. } => assert_eq!(
                points, want,
                "a star asked for {asked} points must be made with {want}"
            ),
            other => panic!("create_star makes a star, not {other:?}"),
        }
    }

    // The only place `STAR_DEFAULT_RATIO` reaches a node.
    match made(&star(5)) {
        NodeKind::Star { inner_ratio, .. } => assert_eq!(inner_ratio, STAR_DEFAULT_RATIO),
        other => panic!("not {other:?}"),
    }
}

/// `create_image_in_box` neither fits nor locks the aspect: the layer is the box that
/// was drawn, with a fixture `create_image` would shrink.
///
/// flip: insert `fitted_image_size` into `create_image_in_box` → fails at the layer size
#[test]
fn a_dragged_image_takes_the_box_it_was_drawn_at() {
    let mut ids = IdSource::new(1);
    let (node, parent) = (ids.mint(), ids.mint());
    // A 400x200 image drawn into a 50x300 box: a fit would change both numbers.
    let intrinsic = Size::new(400.0, 200.0);
    let drawn = Size::new(50.0, 300.0);
    assert_ne!(
        fitted_image_size(intrinsic, drawn),
        drawn,
        "the fixture must be a box the fit rule would argue with, or this \
         test cannot see the difference it is about"
    );

    let tx = create_image_in_box(
        node,
        parent,
        0,
        Affine::IDENTITY,
        Point::new(10.0, 20.0),
        Point::new(60.0, 320.0),
        ImageId("sha256:x".into()),
        Some(entry_of(400, 200)),
        None,
    );
    match made(&tx) {
        NodeKind::Rect { size, .. } => assert_eq!(
            size, drawn,
            "the layer is the box that was drawn, at the aspect it was drawn"
        ),
        other => panic!("an image is placed as a Rect, not {other:?}"),
    }
}

/// `create_ellipse` passes the dragged box through to an `Ellipse`; the shared
/// `local_box` is covered by `create_shape_box_is_order_independent`.
#[test]
fn an_ellipse_is_made_at_the_box_it_was_dragged() {
    let mut ids = IdSource::new(1);
    let tx = create_ellipse(
        ids.mint(),
        ids.mint(),
        0,
        Affine::IDENTITY,
        Point::new(60.0, 320.0),
        Point::new(10.0, 20.0),
        Color::BLACK,
    );
    match made(&tx) {
        NodeKind::Ellipse { size, .. } => assert_eq!(size, Size::new(50.0, 300.0)),
        other => panic!("create_ellipse makes an ellipse, not {other:?}"),
    }
}

/// The transaction adds the image bytes before the node that refers to them, so its
/// inverse removes the node first.
#[test]
fn placing_adds_the_image_before_the_layer_and_fills_with_it() {
    let mut ids = IdSource::new(2);
    let (node, parent) = (ids.mint(), ids.mint());
    let id = ImageId("sha256:y".into());
    let tx = create_image(
        node,
        parent,
        0,
        Affine::IDENTITY,
        Point::ZERO,
        Size::new(500.0, 500.0),
        id.clone(),
        Some(entry_of(10, 10)),
        Size::new(10.0, 10.0),
        Some("hero.png".into()),
    );
    assert!(
        matches!(tx.0[0], Operation::AddImage { .. }),
        "the bytes come first: {:?}",
        tx.0[0]
    );
    assert!(matches!(tx.0[1], Operation::CreateNode { .. }));
    let Operation::SetFills { fills, .. } = &tx.0[2] else {
        panic!("expected the fill last, got {:?}", tx.0[2]);
    };
    assert_eq!(fills[0].brush, crate::image_brush(id).into());

    // The layer is named for the file.
    let Operation::CreateNode { name, .. } = &tx.0[1] else {
        unreachable!()
    };
    assert_eq!(name.as_deref(), Some("hero.png"));
}

/// Placing a file the document already carries adds a reference, not a second copy.
#[test]
fn placing_an_image_the_document_already_has_adds_no_second_copy() {
    let mut ids = IdSource::new(3);
    let (node, parent) = (ids.mint(), ids.mint());
    let id = ImageId("sha256:z".into());
    let tx = create_image(
        node,
        parent,
        0,
        Affine::IDENTITY,
        Point::ZERO,
        Size::new(500.0, 500.0),
        id.clone(),
        None,
        Size::new(10.0, 10.0),
        None,
    );
    assert_eq!(tx.0.len(), 2, "expected no AddImage: {:?}", tx.0);
    assert!(!tx.0.iter().any(|o| matches!(o, Operation::AddImage { .. })));
    let Operation::SetFills { fills, .. } = &tx.0[1] else {
        unreachable!()
    };
    assert_eq!(
        fills[0].brush,
        crate::image_brush(id).into(),
        "the second layer still points at the one copy"
    );
}

/// A text layer wraps only once its line outruns the width it is given, and then
/// at that width with a height that follows the text (`AutoHeight`, never `Fixed`);
/// a line exactly as wide stays auto. `\r\n` goes in as one break.
///
/// flip: `>=` for `>` in `text_layer_kind`'s wrap test → fails at "exactly fits".
#[test]
fn a_text_layer_wraps_only_once_it_outruns_the_width() {
    let parts = crate::TextParts::default();
    let sizing = |kind: &NodeKind| match kind {
        NodeKind::Text { sizing, .. } => *sizing,
        _ => unreachable!(),
    };
    let (kind, size, _) = text_layer_kind("a line of some words", parts.clone(), None);
    assert_eq!(sizing(&kind), TextSizing::Auto);
    let w = size.width;
    assert!(w > 10.0, "{size:?}");
    let (kind, ..) = text_layer_kind("a line of some words", parts.clone(), Some(w));
    assert_eq!(sizing(&kind), TextSizing::Auto, "exactly fits");
    let (kind, ..) = text_layer_kind("a line of some words", parts.clone(), Some(w - 1.0));
    assert_eq!(
        sizing(&kind),
        TextSizing::AutoHeight(w - 1.0),
        "a hair over wraps"
    );
    let (kind, ..) = text_layer_kind("one\r\ntwo", parts, None);
    let NodeKind::Text { content, .. } = kind else {
        unreachable!()
    };
    assert_eq!(content, "one\ntwo");
}
