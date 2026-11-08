//! A layer's credit (`ondin_core::credit`): kept through save, copy and ungroup,
//! refused when malformed, dropped when its picture is replaced.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Credit, CreditKind, Document, Fill, History, IdSource, NodeId, NodeKind, OpError,
    Operation, Resolved, Transaction, build, io, keyed_by_position,
};

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn credit(kind: CreditKind) -> Credit {
    Credit {
        kind,
        text: "Photo by Ana Ruiz on Unsplash".into(),
        author: "Ana Ruiz".into(),
        author_url: "https://unsplash.com/@ana".into(),
        provider: "Unsplash".into(),
        provider_url: "https://unsplash.com".into(),
        source_url: "https://unsplash.com/photos/abc".into(),
        licence: "Unsplash License".into(),
        licence_url: "https://unsplash.com/license".into(),
        required: true,
        inserted: "2026-11-08".into(),
    }
}

struct F {
    doc: Document,
    hist: History,
    ids: IdSource,
    root: NodeId,
}

impl F {
    fn new() -> Self {
        let mut ids = IdSource::new(0xC4ED);
        let root = ids.mint();
        Self {
            doc: Document::new(root),
            hist: History::new(),
            ids,
            root,
        }
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate((5.0, 5.0))),
            name: None,
        }]));
        id
    }

    fn commit(&mut self, tx: Transaction) {
        self.hist.commit(&mut self.doc, tx).expect("commit");
    }

    fn set_credit(&mut self, id: NodeId, c: Credit) {
        self.commit(Transaction(vec![Operation::SetCredit {
            id,
            credit: Some(Box::new(c)),
        }]));
    }
}

/// A credit survives a save and load byte for byte; a layer without one writes no key.
#[test]
fn a_credit_round_trips_through_the_file() {
    let mut f = F::new();
    let a = f.add(f.root, rect());
    let b = f.add(f.root, rect());
    f.set_credit(a, credit(CreditKind::Photo));
    let bytes = io::save(&f.doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert_eq!(
        text.matches("\"credit\"").count(),
        1,
        "only the credited layer"
    );
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(
        loaded.get(a).unwrap().credit(),
        Some(&credit(CreditKind::Photo))
    );
    assert_eq!(loaded.get(b).unwrap().credit(), None);
    assert_eq!(io::save(&loaded).unwrap(), bytes, "byte-stable");
}

/// A hand-edited file's non-web address loads emptied, the document still opens.
#[test]
fn a_bad_address_in_a_file_loads_emptied() {
    let mut f = F::new();
    let a = f.add(f.root, rect());
    f.set_credit(a, credit(CreditKind::Photo));
    let text = String::from_utf8(io::save(&f.doc).unwrap()).unwrap();
    let edited = text.replace("https://unsplash.com/@ana", "file:///etc/passwd");
    let loaded = io::load(edited.as_bytes()).unwrap();
    let c = loaded.get(a).unwrap().credit().unwrap();
    assert_eq!(c.author_url, "");
    assert_eq!(c.provider_url, "https://unsplash.com");
}

/// The operation refuses a non-web address and undoes to no credit.
#[test]
fn set_credit_refuses_a_bad_address_and_undoes() {
    let mut f = F::new();
    let a = f.add(f.root, rect());
    let mut bad = credit(CreditKind::Photo);
    bad.source_url = "javascript:alert(1)".into();
    let err = f.doc.apply(&Transaction(vec![Operation::SetCredit {
        id: a,
        credit: Some(Box::new(bad)),
    }]));
    assert!(matches!(err, Err(OpError::BadCredit)));
    f.set_credit(a, credit(CreditKind::Icon));
    assert!(f.doc.get(a).unwrap().credit().is_some());
    f.hist.undo(&mut f.doc).unwrap();
    assert_eq!(f.doc.get(a).unwrap().credit(), None);
}

/// A copy keeps the credit, and ungrouping an icon gives it to each part.
/// flip: drop the `SetCredit` push in `build::ungroup` → fails at the children's assert.
#[test]
fn copies_and_ungrouped_parts_keep_the_credit() {
    let mut f = F::new();
    let a = f.add(f.root, rect());
    let b = f.add(f.root, rect());
    let (tx, g) = build::group(&f.doc, &Resolved::rebuild(&f.doc), &mut f.ids, &[a, b]).unwrap();
    f.commit(tx);
    let icon = credit(CreditKind::Icon);
    f.set_credit(g, icon.clone());

    let template = f.doc.capture_subtree(g).unwrap();
    let (copy, root) = ondin_core::remap_subtree(&template, &mut f.ids).unwrap();
    let copied = copy.iter().find(|n| n.id() == root).unwrap();
    assert_eq!(copied.credit(), Some(&icon), "a copy keeps it");

    let tx = build::ungroup(&f.doc, &Resolved::rebuild(&f.doc), g).unwrap();
    f.commit(tx);
    assert_eq!(f.doc.get(a).unwrap().credit(), Some(&icon));
    assert_eq!(f.doc.get(b).unwrap().credit(), Some(&icon));
}

/// A main `m` holding a rect, and an instance of it made by the copy path; the
/// rect and its copy.
fn main_and_instance(f: &mut F) -> (NodeId, NodeId) {
    let m = f.add(
        f.root,
        NodeKind::Artboard {
            size: Size::new(100.0, 100.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
    );
    let a = f.add(m, rect());
    f.commit(Transaction(vec![Operation::SetComponent {
        id: m,
        component: true,
    }]));
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut f.ids,
        &[build::Placement {
            nodes: f.doc.capture_subtree(m).unwrap(),
            parent: f.root,
            index: None,
        }],
        Default::default(),
    );
    f.commit(tx);
    let ia = f.doc.get(made[0]).unwrap().children()[0];
    (a, ia)
}

/// `ops` and what they owe the instances — the commit's assembly.
fn commit_propagated(f: &mut F, ops: Vec<Operation>) {
    let mut tx = Transaction(ops);
    let follows = ondin_core::propagate::propagate(&f.doc, &tx);
    tx.0.extend(follows);
    f.commit(tx);
}

/// A credit written on a main reaches its copy, and dropped there drops it on
/// the copy; a copy's own credit is an override the main's edit leaves alone.
/// flip: `SetCredit` back to `Mode::Skip` in `propagate::mode` → fails at the
/// first copy assertion.
#[test]
fn a_mains_credit_reaches_its_copies() {
    let mut f = F::new();
    let (a, ia) = main_and_instance(&mut f);
    let photo = credit(CreditKind::Photo);
    commit_propagated(
        &mut f,
        vec![Operation::SetCredit {
            id: a,
            credit: Some(Box::new(photo.clone())),
        }],
    );
    assert_eq!(f.doc.get(ia).unwrap().credit(), Some(&photo));
    commit_propagated(
        &mut f,
        vec![Operation::SetCredit {
            id: a,
            credit: None,
        }],
    );
    assert_eq!(f.doc.get(ia).unwrap().credit(), None);

    let mut own = credit(CreditKind::Photo);
    own.text = "Photo by Tomás Vidal on Unsplash".into();
    commit_propagated(
        &mut f,
        vec![Operation::SetCredit {
            id: ia,
            credit: Some(Box::new(own.clone())),
        }],
    );
    commit_propagated(
        &mut f,
        vec![Operation::SetCredit {
            id: a,
            credit: Some(Box::new(photo)),
        }],
    );
    assert_eq!(
        f.doc.get(ia).unwrap().credit(),
        Some(&own),
        "an override stays"
    );
}

/// Flattening a credited icon's parts keeps the credit on the union.
/// flip: drop the `SetCredit` push in `build::flatten_union` → fails here.
#[test]
fn a_flattened_icon_keeps_its_credit() {
    let mut f = F::new();
    let a = f.add(f.root, rect());
    let b = f.add(f.root, rect());
    let icon = credit(CreditKind::Icon);
    f.set_credit(b, icon.clone());
    let (tx, made) = build::flatten(&f.doc, &Resolved::rebuild(&f.doc), &mut f.ids, &[a, b])
        .expect("two rects flatten");
    f.commit(tx);
    assert_eq!(f.doc.get(made).unwrap().credit(), Some(&icon));
}

/// A credit that will not read — a missing field, a kind from a later build —
/// is dropped on load, and the document opens.
#[test]
fn an_unreadable_credit_is_dropped_not_refused() {
    let mut f = F::new();
    let a = f.add(f.root, rect());
    f.set_credit(a, credit(CreditKind::Photo));
    let text = String::from_utf8(io::save(&f.doc).unwrap()).unwrap();
    let edited = text.replace("\"photo\"", "\"video\"");
    assert_ne!(edited, text, "the fixture: the kind is in the file");
    let loaded = io::load(edited.as_bytes()).expect("the document opens");
    assert_eq!(loaded.get(a).unwrap().credit(), None);
}

/// Replacing the picture drops a photo credit and keeps an icon's.
#[test]
fn replacing_the_picture_drops_a_photo_credit_only() {
    let mut f = F::new();
    let photo = f.add(f.root, rect());
    let icon = f.add(f.root, rect());
    let fills = keyed_by_position([Fill {
        brush: Brush::Solid(Color::from_rgba8(9, 9, 9, 255)).into(),
        visible: true,
    }]);
    for id in [photo, icon] {
        f.commit(Transaction(vec![Operation::SetFills {
            id,
            fills: fills.clone(),
        }]));
    }
    f.set_credit(photo, credit(CreditKind::Photo));
    f.set_credit(icon, credit(CreditKind::Icon));
    let pic = ondin_core::ImageId("sha:other".into());
    for id in [photo, icon] {
        let tx = build::replace_image_fill(&f.doc, id, 0, &pic, None).unwrap();
        f.commit(tx);
    }
    assert_eq!(f.doc.get(photo).unwrap().credit(), None);
    assert!(f.doc.get(icon).unwrap().credit().is_some());
}
