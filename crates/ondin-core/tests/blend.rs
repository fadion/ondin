//! Blend modes in the model: the kind's default, the op's refusals, the file.

use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::{
    BlendMode, Document, IdSource, NodeId, NodeKind, OpError, Operation, Transaction, build, io,
};

struct Fixture {
    doc: Document,
    frame: NodeId,
    group: NodeId,
    rect: NodeId,
}

/// A frame holding a group holding a rect.
fn fixture() -> Fixture {
    let mut ids = IdSource::new(3);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, group, rect) = (ids.mint(), ids.mint(), ids.mint());
    let page = doc.first_page();
    let create = |id, parent, kind| Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind,
        transform: None,
        name: None,
    };
    doc.apply(&Transaction(vec![
        create(
            frame,
            page,
            NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
        ),
        create(group, frame, NodeKind::Group),
        create(
            rect,
            group,
            NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
        ),
    ]))
    .unwrap();
    Fixture {
        doc,
        frame,
        group,
        rect,
    }
}

fn blend(doc: &Document, id: NodeId) -> BlendMode {
    doc.get(id).unwrap().blend()
}

/// Frames and groups start passing through; every other kind starts Normal.
#[test]
fn a_new_layer_takes_its_kinds_default() {
    let f = fixture();
    assert_eq!(blend(&f.doc, f.frame), BlendMode::PassThrough);
    assert_eq!(blend(&f.doc, f.group), BlendMode::PassThrough);
    assert_eq!(blend(&f.doc, f.rect), BlendMode::Normal);
}

/// Pass through on a rect is refused, and leaves the rect as it was.
#[test]
fn pass_through_is_refused_on_a_leaf() {
    let mut f = fixture();
    let err = f
        .doc
        .apply(&Transaction(vec![Operation::SetBlend {
            id: f.rect,
            blend: BlendMode::PassThrough,
        }]))
        .unwrap_err();
    assert!(matches!(err, OpError::WrongKindForOp), "got {err:?}");
    assert_eq!(blend(&f.doc, f.rect), BlendMode::Normal);
}

/// The file leaves out each layer's default and keeps everything else — Normal
/// on a group included, since it isolates — and reads back what it wrote.
#[test]
fn the_file_skips_each_kinds_default_and_round_trips_the_rest() {
    let mut f = fixture();
    let before = io::save(&f.doc).unwrap();
    assert!(
        !String::from_utf8_lossy(&before).contains("\"blend\""),
        "an untouched document writes no blend key"
    );
    f.doc
        .apply(&Transaction(vec![
            Operation::SetBlend {
                id: f.group,
                blend: BlendMode::Normal,
            },
            Operation::SetBlend {
                id: f.rect,
                blend: BlendMode::ColorDodge,
            },
        ]))
        .unwrap();
    let bytes = io::save(&f.doc).unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(text.matches("\"blend\"").count(), 2, "{text}");
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(blend(&loaded, f.frame), BlendMode::PassThrough);
    assert_eq!(blend(&loaded, f.group), BlendMode::Normal);
    assert_eq!(blend(&loaded, f.rect), BlendMode::ColorDodge);
    assert_eq!(io::save(&loaded).unwrap(), bytes, "stable bytes");
}

/// `Shift`+`3` sets each layer's default — Pass through on the group, Normal on
/// the rect — and a pick lands on the outermost members alone.
#[test]
fn set_blend_all_lands_pass_through_as_normal_on_a_leaf() {
    let mut f = fixture();
    let multiply = build::set_blend_all(&f.doc, &[f.rect], BlendMode::Multiply);
    f.doc.apply(&multiply).unwrap();
    f.doc
        .apply(&Transaction(vec![Operation::SetBlend {
            id: f.group,
            blend: BlendMode::Screen,
        }]))
        .unwrap();
    for id in [f.group, f.rect] {
        let tx = build::set_blend_all(&f.doc, &[id], BlendMode::PassThrough);
        f.doc.apply(&tx).unwrap();
    }
    assert_eq!(blend(&f.doc, f.group), BlendMode::PassThrough);
    assert_eq!(blend(&f.doc, f.rect), BlendMode::Normal);

    let both = build::set_blend_all(&f.doc, &[f.group, f.rect], BlendMode::Overlay);
    assert_eq!(both.0.len(), 1, "the rect is inside the group: {both:?}");
}

/// *Mixed* is a disagreement among the outermost members.
#[test]
fn shared_blend_reports_a_disagreement_as_none() {
    let mut f = fixture();
    assert_eq!(
        build::shared_blend(&f.doc, &[f.frame]),
        Some(BlendMode::PassThrough)
    );
    let group = f.group;
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: NodeId { actor: 3, seq: 99 },
            parent: f.frame,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        }]))
        .unwrap();
    let other = NodeId { actor: 3, seq: 99 };
    assert_eq!(
        build::shared_blend(&f.doc, &[group, other]),
        Some(BlendMode::PassThrough)
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetBlend {
            id: other,
            blend: BlendMode::Hue,
        }]))
        .unwrap();
    assert_eq!(build::shared_blend(&f.doc, &[group, other]), None);
}

/// A CSS keyword reads back as the mode that wrote it, for every mode but Pass
/// through, which has none.
#[test]
fn css_keywords_round_trip() {
    for m in BlendMode::ALL.into_iter().skip(1) {
        assert_eq!(BlendMode::from_css(m.css()), Some(m), "{m:?}");
    }
    assert_eq!(BlendMode::from_css("plus-lighter"), None);
}

/// A copied blend lands relative to the copied layer's default: a frame's
/// isolating Normal isolates the group it is pasted on, and a rectangle's Normal
/// leaves that group passing through.
/// flip: `paste_properties` mapping every Normal to Pass through → fails at "the
/// frame's Normal isolates".
#[test]
fn a_pasted_blend_lands_relative_to_the_copied_layers_default() {
    let mut f = fixture();
    let mut ids = IdSource::new(99);
    let rect = build::properties_of(&f.doc, f.rect).expect("a rect's properties");
    f.doc
        .apply(&build::paste_properties(
            &f.doc,
            &[f.group],
            &rect,
            &mut ids,
        ))
        .unwrap();
    assert_eq!(
        blend(&f.doc, f.group),
        BlendMode::PassThrough,
        "a leaf's Normal"
    );

    f.doc
        .apply(&Transaction(vec![Operation::SetBlend {
            id: f.frame,
            blend: BlendMode::Normal,
        }]))
        .unwrap();
    let frame = build::properties_of(&f.doc, f.frame).expect("a frame's properties");
    f.doc
        .apply(&build::paste_properties(
            &f.doc,
            &[f.group],
            &frame,
            &mut ids,
        ))
        .unwrap();
    assert_eq!(
        blend(&f.doc, f.group),
        BlendMode::Normal,
        "the frame's Normal isolates"
    );
}
