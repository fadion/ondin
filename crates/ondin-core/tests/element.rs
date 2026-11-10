//! A layer's HTML element: what each kind is offered, the rules that keep the
//! HTML valid, a set's one element on all its roots, and the save format.

use ondin_core::element::{self, HtmlElement};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::variant::{VariantProp, VariantSet};
use ondin_core::{Document, IdSource, NodeId, NodeKind, Operation, TextSizing, Transaction, io};

fn frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(100.0, 100.0),
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

fn create(id: NodeId, parent: NodeId, index: usize, kind: NodeKind, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::IDENTITY),
        name: Some(name.into()),
    }
}

fn set(id: NodeId, e: HtmlElement) -> Operation {
    Operation::SetElement {
        id,
        element: Some(e),
    }
}

/// A page holding *Site* (a frame) with *Card* (a frame holding *Terms*, a
/// text, and *Copy*, a text of two paragraphs) and *Content* (a frame), and a
/// rect *Logo*.
struct F {
    doc: Document,
    site: NodeId,
    card: NodeId,
    terms: NodeId,
    copy: NodeId,
    content: NodeId,
    logo: NodeId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xE1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let [site, card, terms, copy, content, logo] = [(); 6].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(site, page, 0, frame(), "Site"),
        create(card, site, 0, frame(), "Card"),
        create(terms, card, 0, text("Terms"), "Terms"),
        create(copy, card, 1, text("One\nTwo"), "Copy"),
        create(content, site, 1, frame(), "Content"),
        create(
            logo,
            site,
            2,
            NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            "Logo",
        ),
    ]))
    .unwrap();
    F {
        doc,
        site,
        card,
        terms,
        copy,
        content,
        logo,
    }
}

/// Frames take landmarks, lists, buttons and links; texts headings, buttons
/// and links; a shape nothing, and the op refuses what the kind is not offered.
#[test]
fn each_kind_is_offered_its_own_elements() {
    let mut f = fixture();
    let kind = |id| f.doc.get(id).unwrap().kind().clone();
    assert!(
        element::offered(&kind(f.card))
            .unwrap()
            .contains(&HtmlElement::Nav)
    );
    assert!(
        !element::offered(&kind(f.card))
            .unwrap()
            .contains(&HtmlElement::H1)
    );
    assert!(
        element::offered(&kind(f.terms))
            .unwrap()
            .contains(&HtmlElement::H2)
    );
    assert!(element::offered(&kind(f.logo)).is_none());
    assert!(element::kind_refusal(&kind(f.logo)).is_some());
    assert!(
        f.doc
            .apply(&Transaction(vec![set(f.logo, HtmlElement::Nav)]))
            .is_err()
    );
    assert!(
        f.doc
            .apply(&Transaction(vec![set(f.terms, HtmlElement::Nav)]))
            .is_err()
    );
    assert!(
        f.doc
            .apply(&Transaction(vec![Operation::SetHref {
                id: f.card,
                href: "javascript:alert(1)".into(),
            }]))
            .is_err()
    );
}

/// Interactive content does not nest, either way round, and a button holds
/// nothing with an element of its own.
///
/// flip: `refusal` skipping the layers inside → fails at "a button can't hold
/// a link".
#[test]
fn a_button_and_a_link_do_not_nest() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![set(f.terms, HtmlElement::Link)]))
        .unwrap();
    let why = element::refusal(&f.doc, f.card, HtmlElement::Button).unwrap();
    assert_eq!(
        why,
        "A button can't hold a link — “Terms”, inside it, is one"
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetElement {
            id: f.terms,
            element: None,
        }]))
        .unwrap();
    assert_eq!(element::refusal(&f.doc, f.card, HtmlElement::Button), None);
    f.doc
        .apply(&Transaction(vec![set(f.card, HtmlElement::Button)]))
        .unwrap();
    let why = element::refusal(&f.doc, f.terms, HtmlElement::Link).unwrap();
    assert!(why.starts_with("Inside a button"), "{why}");
}

/// A text of several paragraphs may be a heading, a button or a link: the
/// export writes its paragraphs as block spans inside one.
#[test]
fn a_heading_may_hold_several_paragraphs() {
    let f = fixture();
    for e in [HtmlElement::H1, HtmlElement::Button, HtmlElement::Link] {
        assert_eq!(element::refusal(&f.doc, f.copy, e), None, "{e:?}");
    }
}

/// A top-level frame holds one main, never inside a header or the like, and a
/// header holds no header.
#[test]
fn a_page_has_one_main() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![set(f.content, HtmlElement::Main)]))
        .unwrap();
    assert_eq!(
        element::refusal(&f.doc, f.card, HtmlElement::Main).as_deref(),
        Some("A page has one main — “Content” is it")
    );
    assert_eq!(element::refusal(&f.doc, f.content, HtmlElement::Main), None);
    assert!(element::refusal(&f.doc, f.site, HtmlElement::Header).is_some());
    f.doc
        .apply(&Transaction(vec![
            Operation::SetElement {
                id: f.content,
                element: None,
            },
            set(f.site, HtmlElement::Header),
        ]))
        .unwrap();
    assert!(element::refusal(&f.doc, f.card, HtmlElement::Main).is_some());
    assert!(element::refusal(&f.doc, f.card, HtmlElement::Footer).is_some());
}

/// A main sits only inside plain boxes: a list or a link around it is refused
/// as a landmark is, and neither may be set around one.
/// flip: `bars_main` listing only the landmarks → fails at "inside a list".
#[test]
fn the_main_sits_only_in_plain_boxes() {
    for around in [HtmlElement::List, HtmlElement::Link, HtmlElement::Section] {
        let mut f = fixture();
        f.doc
            .apply(&Transaction(vec![set(f.site, around)]))
            .unwrap();
        assert!(
            element::refusal(&f.doc, f.card, HtmlElement::Main).is_some(),
            "inside a list, a link or a section: {around:?}"
        );
        let mut f = fixture();
        f.doc
            .apply(&Transaction(vec![set(f.card, HtmlElement::Main)]))
            .unwrap();
        assert!(
            element::refusal(&f.doc, f.site, around).is_some(),
            "around the main: {around:?}"
        );
    }
}

/// A list's children are its items: Automatic reads `li` there. Elsewhere a
/// text reads `div`, the box its paragraphs sit in, and a shape no tag.
#[test]
fn a_lists_children_are_items() {
    let mut f = fixture();
    let tag = |f: &F, id| element::automatic_tag(&f.doc, f.doc.get(id).unwrap());
    assert_eq!(tag(&f, f.terms), Some("div"));
    assert_eq!(tag(&f, f.logo), None, "a shape is an svg, an img or a box");
    f.doc
        .apply(&Transaction(vec![set(f.card, HtmlElement::List)]))
        .unwrap();
    assert_eq!(tag(&f, f.terms), Some("li"));
    assert_eq!(tag(&f, f.card), Some("div"));
}

/// Element and address save and load; a tag this build does not know, and an
/// address no link takes, are dropped on load rather than refusing the file.
#[test]
fn the_element_and_its_address_round_trip() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![
            set(f.card, HtmlElement::Link),
            Operation::SetHref {
                id: f.card,
                href: "/signup".into(),
            },
        ]))
        .unwrap();
    let bytes = io::save(&f.doc).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("\"element\": \"a\"") || text.contains("\"element\":\"a\""));
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(
        loaded.get(f.card).unwrap().element(),
        Some(HtmlElement::Link)
    );
    assert_eq!(loaded.get(f.card).unwrap().href(), "/signup");
    let odd = text
        .replace("\"a\"", "\"marquee\"")
        .replace("/signup", "javascript:x");
    let loaded = io::load(odd.as_bytes()).unwrap();
    assert_eq!(loaded.get(f.card).unwrap().element(), None);
    assert_eq!(loaded.get(f.card).unwrap().href(), "");
}

/// A set of two variants under the page, marked or not.
fn set_of_two(marked: bool) -> (Document, NodeId, [NodeId; 2]) {
    use ondin_core::variant::{self, Interaction};
    let mut ids = IdSource::new(0xE2);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let [s, rest, hover] = [(); 3].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(s, page, 0, frame(), "Button"),
        create(rest, s, 0, frame(), "Rest"),
        create(hover, s, 1, frame(), "Hover"),
        Operation::SetComponent {
            id: rest,
            component: true,
        },
        Operation::SetComponent {
            id: hover,
            component: true,
        },
        Operation::SetVariantSet {
            id: s,
            set: Some(VariantSet {
                props: vec![VariantProp::new(
                    "State",
                    vec!["Default".into(), "Hover".into()],
                )],
            }),
        },
        Operation::SetVariant {
            id: rest,
            values: vec!["Default".into()],
        },
        Operation::SetVariant {
            id: hover,
            values: vec!["Hover".into()],
        },
    ]))
    .unwrap();
    if marked {
        let tx = variant::tie_state(&doc, s, 0, 1, Some(Interaction::Hover)).unwrap();
        doc.apply(&tx).unwrap();
    }
    (doc, s, [rest, hover])
}

/// A set first given states makes its roots buttons; one choice on any root
/// writes every root.
#[test]
fn a_set_has_one_element_on_every_root() {
    let (doc, s, [rest, hover]) = set_of_two(true);
    for r in [rest, hover] {
        assert_eq!(doc.get(r).unwrap().element(), Some(HtmlElement::Button));
    }
    let ops = element::set_ops(&doc, hover, Some(HtmlElement::Link));
    assert_eq!(ops.len(), 2, "both roots: {ops:?}");
    assert_eq!(
        element::set_ops(&doc, s, Some(HtmlElement::Button)).len(),
        0
    );
}

/// A v14 file's marked set keeps the element it was written as, now on its
/// roots and their instances: absent (Button) and Link move, Plain is
/// Automatic, and an unmarked set's is dropped.
///
/// flip: `set_element_onto_roots` skipping the absent key → fails at
/// "absent is a button"; skipping the linked copies → fails at "an instance
/// follows its main".
#[test]
fn a_v14_sets_element_moves_onto_its_roots() {
    let (mut doc, s, [rest, hover]) = set_of_two(true);
    let page = doc.first_page();
    let (tx, copy) =
        ondin_core::build::place_instance(&doc, &mut IdSource::new(0xE9), s, page, None).unwrap();
    doc.apply(&tx).unwrap();
    let clear = Transaction(
        [rest, hover, copy]
            .map(|id| Operation::SetElement { id, element: None })
            .to_vec(),
    );
    doc.apply(&clear).unwrap();
    let saved = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    value["schema_version"] = 14.into();
    let at = |value: &serde_json::Value| {
        value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|n| n["id"] == s.to_wire())
            .unwrap()
    };
    let i = at(&value);
    let with = |element: Option<&str>| {
        let mut v = value.clone();
        if let Some(e) = element {
            v["nodes"][i]["set"]["element"] = e.into();
        }
        io::load(serde_json::to_string(&v).unwrap().as_bytes()).unwrap()
    };
    let loaded = with(None);
    assert_eq!(
        loaded.get(rest).unwrap().element(),
        Some(HtmlElement::Button),
        "absent is a button"
    );
    assert_eq!(
        loaded.get(hover).unwrap().element(),
        Some(HtmlElement::Button)
    );
    assert_eq!(
        loaded.get(copy).unwrap().element(),
        Some(HtmlElement::Button),
        "an instance follows its main, or it reads as an override"
    );
    assert!(
        ondin_core::reset::overrides(&loaded, copy).is_empty(),
        "and carries none"
    );
    assert_eq!(
        with(Some("link")).get(rest).unwrap().element(),
        Some(HtmlElement::Link)
    );
    assert_eq!(with(Some("plain")).get(rest).unwrap().element(), None);

    let (doc, s2, [rest2, _]) = set_of_two(false);
    let saved = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();
    value["schema_version"] = 14.into();
    let _ = s2;
    let loaded = io::load(serde_json::to_string(&value).unwrap().as_bytes()).unwrap();
    assert_eq!(loaded.get(rest2).unwrap().element(), None, "unmarked");
}
