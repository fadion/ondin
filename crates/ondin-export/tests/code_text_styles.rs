//! Code export's text styles: one class per text style the export uses, in
//! `text-styles.css`, worn by each text beside its own class, whose rule keeps
//! only what the text sets differently.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::style::{self, StyleSlot, StyleValue};
use ondin_core::{
    Document, IdSource, Length, NodeId, NodeKind, Operation, Resolved, TextSizing, TextStyle,
    Transaction,
};
use ondin_export::code::{CodeExport, Target};

fn text(content: &str, style: TextStyle) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(style),
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

fn heading() -> TextStyle {
    TextStyle {
        font_size: 24.0,
        weight: 700,
        ..Default::default()
    }
}

fn body() -> TextStyle {
    TextStyle {
        font_size: 14.0,
        line_height: Some(Length::Px(20.0)),
        ..Default::default()
    }
}

/// A board holding two texts in *Heading / Large* (an automatic line height),
/// one in *Body*, one in no style, and a text named as the heading's class
/// would be.
fn fixture() -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x7E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let board = ids.mint();
    let [title, lede, copy, loose, clash] = [(); 5].map(|_| ids.mint());
    let (h, b) = (ids.mint_style(), ids.mint_style());
    let place = |id, index, kind, y: f64, name: &str| Operation::CreateNode {
        id,
        parent: board,
        index,
        kind,
        transform: Some(Affine::translate((10.0, y))),
        name: Some(name.into()),
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 300.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Board".into()),
        },
        place(title, 0, text("Title", heading()), 10.0, "Title"),
        place(lede, 1, text("Lede", heading()), 50.0, "Lede"),
        place(copy, 2, text("Copy", body()), 90.0, "Copy"),
        place(loose, 3, text("Loose", body()), 130.0, "Loose"),
        place(
            clash,
            4,
            text("Clash", TextStyle::default()),
            170.0,
            "Text Heading Large",
        ),
        style::add_op(h, "Heading/Large", StyleValue::Text(Box::new(heading()))),
        style::add_op(b, "Body", StyleValue::Text(Box::new(body()))),
        style::bind_op(title, StyleSlot::Text, Some(h)),
        style::bind_op(lede, StyleSlot::Text, Some(h)),
        style::bind_op(copy, StyleSlot::Text, Some(b)),
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, board)
}

fn file(out: &CodeExport, path: &str) -> String {
    String::from_utf8(
        out.file(path)
            .unwrap_or_else(|| panic!("no {path}"))
            .to_vec(),
    )
    .unwrap()
}

/// The rule for `.class` in `css`, its body only.
fn rule<'a>(css: &'a str, class: &str) -> &'a str {
    let head = format!(".{class} {{\n");
    let at = css
        .find(&head)
        .unwrap_or_else(|| panic!("no {head} in {css}"))
        + head.len();
    &css[at..at + css[at..].find('}').unwrap()]
}

/// The heading's class declares its face once, its automatic line height as
/// laid; its texts wear it before their own class, whose rules leave the face
/// out. A text in no style keeps its whole face.
///
/// flip: `TextClass::own` returning the style unchanged → fails at "the
/// title's rule leaves the face to the class".
#[test]
fn html_writes_a_class_per_text_style_used() {
    let (doc, res, board) = fixture();
    let out = Target::Html.export(&doc, &res, &[board]);
    let ts = file(&out, "text-styles.css");
    let heading = rule(&ts, "text-heading-large");
    assert!(heading.contains("font-size: 24px;"), "{ts}");
    assert!(heading.contains("font-weight: 700;"), "{ts}");
    assert!(
        heading.contains("line-height: ") && !heading.contains("line-height: normal"),
        "an automatic line height is the px laid: {ts}"
    );
    assert!(
        rule(&ts, "text-body").contains("line-height: 20px;"),
        "{ts}"
    );
    let html = file(&out, "index.html");
    assert!(
        html.contains("class=\"text-heading-large title\""),
        "{html}"
    );
    assert!(html.contains("class=\"text-body copy\""), "{html}");
    assert!(html.contains("class=\"loose\""), "{html}");
    assert!(
        html.find("href=\"text-styles.css\"").unwrap() < html.find("href=\"styles.css\"").unwrap(),
        "the text styles load first, so a text's own value wins: {html}"
    );
    let css = file(&out, "styles.css");
    let title = rule(&css, "title");
    assert!(
        !title.contains("font-size") && !title.contains("font-weight"),
        "the title's rule leaves the face to the class: {title}"
    );
    assert!(
        title.contains("color:"),
        "colour is no part of a text style: {title}"
    );
    assert!(rule(&css, "loose").contains("font-size: 14px;"), "{css}");
    assert!(
        html.contains("class=\"text-heading-large-2\""),
        "a layer's class keeps off a text style's: {html}"
    );
}

/// React imports the file before its module and joins the global class to the
/// module's; Vue imports it too and writes both classes as one string.
#[test]
fn react_and_vue_wear_the_class_beside_their_own() {
    let (doc, res, board) = fixture();
    let react = Target::React.export(&doc, &res, &[board]);
    let app = file(&react, "App.tsx");
    let import = app.find("import \"./text-styles.css\";").expect(&app);
    assert!(import < app.find("App.module.css").unwrap(), "{app}");
    assert!(
        app.contains("className={`text-heading-large ${styles.title}`}"),
        "{app}"
    );
    assert!(app.contains("className={styles.loose}"), "{app}");
    assert!(react.file("text-styles.css").is_some());
    let vue = Target::Vue.export(&doc, &res, &[board]);
    let sfc = file(&vue, "App.vue");
    assert!(sfc.contains("import \"./text-styles.css\";"), "{sfc}");
    assert!(sfc.contains("class=\"text-heading-large title\""), "{sfc}");
}

/// An export whose texts wear no style writes no `text-styles.css` and links none.
#[test]
fn no_text_style_no_file() {
    let (doc, res, board) = fixture();
    let loose = doc.get(board).unwrap().children()[3];
    let out = Target::Html.export(&doc, &res, &[loose]);
    assert!(out.file("text-styles.css").is_none());
    assert!(!file(&out, "index.html").contains("text-styles"));
}

/// *Copy as CSS* carries no stylesheet, so a styled text keeps its whole face.
#[test]
fn copy_as_css_keeps_the_whole_face() {
    let (doc, res, board) = fixture();
    let css = ondin_export::code::css_of(&doc, &res, &[board]);
    assert!(rule(&css, "title").contains("font-size: 24px;"), "{css}");
}

/// A set of two variants whose label is *Body* in one and *Heading / Large* in
/// the other, and an instance of the second; `text_class` picks which variant's
/// label wears which style.
fn set_of_two(both_heading: bool) -> (Document, Resolved, NodeId) {
    use ondin_core::variant::{VariantProp, VariantSet};
    let mut ids = IdSource::new(0x7F);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [
        set,
        small,
        small_label,
        large,
        large_label,
        inst,
        inst_label,
    ] = [(); 7].map(|_| ids.mint());
    let (h, b) = (ids.mint_style(), ids.mint_style());
    let frame = |w, h| NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    };
    let create = |id, parent, index, kind, y: f64, name: &str| Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::translate((10.0, y))),
        name: Some(name.into()),
    };
    let small_style = if both_heading { heading() } else { body() };
    doc.apply(&Transaction(vec![
        create(set, root, 0, frame(200.0, 120.0), 0.0, "Button"),
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp::new(
                    "Size",
                    vec!["Small".into(), "Large".into()],
                )],
            }),
        },
        create(small, set, 0, frame(80.0, 40.0), 10.0, "Small"),
        create(
            small_label,
            small,
            0,
            text("Go", small_style.clone()),
            4.0,
            "Label",
        ),
        create(large, set, 1, frame(80.0, 40.0), 60.0, "Large"),
        create(large_label, large, 0, text("Go", heading()), 4.0, "Label"),
        style::add_op(h, "Heading/Large", StyleValue::Text(Box::new(heading()))),
        style::add_op(b, "Body", StyleValue::Text(Box::new(body()))),
        style::bind_op(
            small_label,
            StyleSlot::Text,
            Some(if both_heading { h } else { b }),
        ),
        style::bind_op(large_label, StyleSlot::Text, Some(h)),
        Operation::SetComponent {
            id: small,
            component: true,
        },
        Operation::SetVariant {
            id: small,
            values: vec!["Small".into()],
        },
        Operation::SetComponent {
            id: large,
            component: true,
        },
        Operation::SetVariant {
            id: large,
            values: vec!["Large".into()],
        },
        create(inst, root, 1, frame(80.0, 40.0), 200.0, "Large"),
        create(inst_label, inst, 0, text("Go", heading()), 4.0, "Label"),
        style::bind_op(inst_label, StyleSlot::Text, Some(h)),
        Operation::SetLink {
            id: inst,
            link: Some(large),
        },
        Operation::SetLink {
            id: inst_label,
            link: Some(large_label),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    (doc, res, inst)
}

/// A merged element's alternatives wear its class too, so where the variants'
/// texts wear different styles the merged label wears none and every rule is
/// whole; where they agree it wears the shared one.
///
/// flip: `merge::merged` keeping the default's class → fails at "no class".
#[test]
fn a_set_whose_texts_differ_in_style_wears_no_class() {
    let (doc, res, inst) = set_of_two(false);
    let out = Target::React.export(&doc, &res, &[inst]);
    assert!(
        out.inlined.is_empty(),
        "the fixture merges: {:?}",
        out.inlined
    );
    let tsx = file(&out, "components/Button.tsx");
    assert!(!tsx.contains("text-"), "no class: {tsx}");
    let css = file(&out, "components/Button.module.css");
    assert!(css.matches("font-size:").count() >= 2, "whole rules: {css}");
    let (doc, res, inst) = set_of_two(true);
    let out = Target::React.export(&doc, &res, &[inst]);
    let tsx = file(&out, "components/Button.tsx");
    assert!(
        tsx.contains("`text-heading-large ${"),
        "the shared class: {tsx}"
    );
}
