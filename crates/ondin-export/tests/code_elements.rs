//! Code export's per-layer HTML elements: the tag each writes, a list's items,
//! phrasing content in a button or a heading, and an element HTML refuses
//! where the layer sits written as Automatic and reported.

use ondin_core::element::HtmlElement as H;
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, Operation, Resolved, TextSizing, Transaction,
};
use ondin_export::code::{CodeExport, Target};

fn frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(100.0, 40.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn text(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::default(),
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

/// A board's tree built from `(parent index, kind, name)` rows, the board
/// itself index 0; then each `(index, element)` set.
fn doc_of(
    rows: &[(usize, NodeKind, &str)],
    set: &[(usize, H)],
) -> (Document, Resolved, Vec<NodeId>) {
    let mut ids = IdSource::new(0xE3);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let mut made = vec![ids.mint()];
    let mut ops = vec![Operation::CreateNode {
        id: made[0],
        parent: page,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 300.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: Some("Board".into()),
    }];
    let mut counts = vec![0usize];
    for (parent, kind, name) in rows {
        let id = ids.mint();
        ops.push(Operation::CreateNode {
            id,
            parent: made[*parent],
            index: counts[*parent],
            kind: kind.clone(),
            transform: Some(Affine::translate((5.0, 5.0))),
            name: Some((*name).into()),
        });
        counts[*parent] += 1;
        made.push(id);
        counts.push(0);
    }
    doc.apply(&Transaction(ops)).unwrap();
    // Set straight on the document: the menu's refusals are not the export's.
    for (i, e) in set {
        doc.apply(&Transaction(vec![Operation::SetElement {
            id: made[*i],
            element: Some(*e),
        }]))
        .unwrap();
    }
    let res = Resolved::rebuild(&doc);
    (doc, res, made)
}

fn file(out: &CodeExport, path: &str) -> String {
    String::from_utf8(out.file(path).unwrap().to_vec()).unwrap()
}

/// A list's plain children are its `li`s and the rest are wrapped in one that
/// draws no box; a link takes its address; a button its type and phrasing
/// content; a heading holds its paragraph as a span.
///
/// flip: `Walk::items` leaving every child as it is → fails at "the text is the
/// item".
#[test]
fn each_element_writes_its_tag() {
    let rect = NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    };
    let (mut doc, _, made) = doc_of(
        &[
            (0, frame(), "Nav"),         // 1
            (1, frame(), "Links"),       // 2
            (2, text("Home"), "Home"),   // 3
            (2, text("Docs"), "Docs"),   // 4
            (2, rect, "Dot"),            // 5
            (0, frame(), "Go"),          // 6
            (6, text("Go"), "Label"),    // 7
            (0, text("Plans"), "Title"), // 8
        ],
        &[
            (1, H::Nav),
            (2, H::List),
            (4, H::Link),
            (6, H::Button),
            (8, H::H2),
        ],
    );
    doc.apply(&Transaction(vec![Operation::SetHref {
        id: made[4],
        href: "/docs".into(),
    }]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = Target::Html.export(&doc, &res, &[made[0]]);
    let html = file(&out, "index.html");
    assert!(html.contains("<nav class=\"nav\">"), "{html}");
    assert!(html.contains("<ul class=\"links\">"), "{html}");
    assert!(
        html.contains("<li class=\"home\">"),
        "the text is the item: {html}"
    );
    assert!(
        html.contains("<li class=\"docs-item\">\n            <a class=\"docs\" href=\"/docs\">"),
        "a link is wrapped in its item: {html}"
    );
    assert!(
        html.contains("<li class=\"dot\">"),
        "a plain box is an item: {html}"
    );
    assert!(
        html.contains("<button class=\"go\" type=\"button\">"),
        "{html}"
    );
    assert!(
        html.contains("<span class=\"label\"><span class=\"label-p\">Go</span></span>"),
        "phrasing in a button: {html}"
    );
    assert!(html.contains("<h2 class=\"title\">"), "{html}");
    assert!(
        html.contains("<span class=\"title-p\">Plans</span>"),
        "{html}"
    );
    let css = file(&out, "styles.css");
    assert!(css.contains("list-style: none;"), "{css}");
    assert!(out.elements.is_empty(), "{:?}", out.elements);

    let react = Target::React.export(&doc, &res, &[made[0]]);
    let app = file(&react, "App.tsx");
    assert!(app.contains("href=\"/docs\""), "{app}");
    assert!(app.contains("<nav className={styles.nav}>"), "{app}");
}

/// A link moved into a button, and a second main, are written as Automatic
/// and reported.
///
/// flip: `Walk::element_of` keeping every element → fails at "the link is a
/// span".
#[test]
fn an_element_html_refuses_there_is_written_as_automatic() {
    let (doc, res, made) = doc_of(
        &[
            (0, frame(), "Go"),          // 1
            (1, text("Terms"), "Terms"), // 2
            (0, frame(), "One"),         // 3
            (0, frame(), "Two"),         // 4
        ],
        &[(1, H::Button), (2, H::Link), (3, H::Main), (4, H::Main)],
    );
    let out = Target::Html.export(&doc, &res, &[made[0]]);
    let html = file(&out, "index.html");
    assert!(!html.contains("<a "), "the link is a span: {html}");
    assert_eq!(html.matches("<main").count(), 1, "{html}");
    assert_eq!(out.elements.len(), 2, "{:?}", out.elements);
    assert!(out.elements[0].contains("“Terms”"), "{:?}", out.elements);
    assert!(file(&out, "report.txt").contains("Written as another element than set:"));
}

/// A main set to Section is written as a `section` in its component's file,
/// and its use, in the page, takes no element of its own.
///
/// flip: `Walk::component` writing the root without `write_element` → fails at
/// "the component's root".
#[test]
fn a_components_root_takes_its_element_in_its_own_file() {
    let (mut doc, _, made) = doc_of(
        &[(0, frame(), "Card"), (1, text("Hi"), "Label")],
        &[(1, H::Section)],
    );
    let (card, label) = (made[1], made[2]);
    let mut ids = IdSource::new(0xE4);
    let (inst, inst_label) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::SetComponent {
            id: card,
            component: true,
        },
        Operation::CreateNode {
            id: inst,
            parent: made[0],
            index: 1,
            kind: frame(),
            transform: Some(Affine::translate((5.0, 100.0))),
            name: Some("Card".into()),
        },
        Operation::CreateNode {
            id: inst_label,
            parent: inst,
            index: 0,
            kind: text("Hi"),
            transform: Some(Affine::translate((5.0, 5.0))),
            name: Some("Label".into()),
        },
        Operation::SetLink {
            id: inst,
            link: Some(card),
        },
        Operation::SetLink {
            id: inst_label,
            link: Some(label),
        },
        Operation::SetElement {
            id: inst,
            element: Some(H::Section),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    assert!(
        out.inlined.is_empty(),
        "the fixture is a use: {:?}",
        out.inlined
    );
    let card = file(&out, "components/Card.tsx");
    assert!(card.contains("<section"), "the component's root: {card}");
    let app = file(&out, "App.tsx");
    assert!(!app.contains("<section"), "{app}");
}
