//! `matching_layers`, *Select matching layers*: the same layer in each copy of a
//! repeated block, across blocks and across page frames.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::{
    Document, IdSource, NodeId, NodeKind, Operation, TextSizing, TextStyle, Transaction,
    matching_layers,
};

struct Fixture {
    ids: IdSource,
    doc: Document,
    page: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut ids = IdSource::new(0x3A7C);
        let page = ids.mint();
        Self {
            doc: Document::new(page),
            ids,
            page,
        }
    }

    /// Append a child of `kind` to `parent`; `name: None` takes the generated one.
    fn add(&mut self, parent: NodeId, kind: NodeKind, name: Option<&str>) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).map_or(0, |n| n.children().len());
        self.doc
            .apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent,
                index,
                kind,
                transform: Some(Affine::IDENTITY),
                name: name.map(str::to_string),
            }]))
            .expect("the fixture builds");
        id
    }

    fn name(&self, id: NodeId) -> &str {
        self.doc.get(id).expect("in the document").name()
    }
}

fn frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(100.0, 100.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: Default::default(),
        corner_smoothing: 0.0,
    }
}

fn text() -> NodeKind {
    NodeKind::Text {
        content: "hi".into(),
        style: Box::new(TextStyle {
            font_family: "Inter".into(),
            font_size: 12.0,
            ..Default::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::AutoHeight(100.0),
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// One card: an optional leading rectangle, then two generated texts ("Text 1"
/// the title, "Text 2" the body). Returns the card and its two texts.
fn card(f: &mut Fixture, list: NodeId, extra: bool) -> (NodeId, NodeId, NodeId) {
    let card = f.add(list, frame(), None);
    if extra {
        f.add(card, rect(), None);
    }
    let title = f.add(card, text(), None);
    let body = f.add(card, text(), None);
    (card, title, body)
}

/// Two screens, each a header group holding a "Title" and a list of two cards.
/// Screen A's second card leads with a rectangle the others lack.
struct Screens {
    f: Fixture,
    titles: Vec<NodeId>,
    bodies: Vec<NodeId>,
    cards: Vec<NodeId>,
    headings: Vec<NodeId>,
    screen_a: NodeId,
}

fn screens() -> Screens {
    let mut f = Fixture::new();
    let page = f.page;
    let (mut titles, mut bodies, mut cards, mut headings) = (vec![], vec![], vec![], vec![]);
    let mut screen_a = page;
    for (i, screen) in ["Screen A", "Screen B"].into_iter().enumerate() {
        let s = f.add(page, frame(), Some(screen));
        if i == 0 {
            screen_a = s;
        }
        let header = f.add(s, NodeKind::Group, Some("Header"));
        headings.push(f.add(header, text(), Some("Title")));
        let list = f.add(s, NodeKind::Group, Some("List"));
        for extra in [false, i == 0] {
            let (c, t, b) = card(&mut f, list, extra);
            cards.push(c);
            titles.push(t);
            bodies.push(b);
        }
    }
    Screens {
        f,
        titles,
        bodies,
        cards,
        headings,
        screen_a,
    }
}

/// A card's title finds every card's title on both screens — past a duplicate's
/// renumbered card name and past an extra leading layer — and no body.
/// flip: `matching_layers` comparing places without the ordinal → fails at the
/// first `matching_layers` assertion, with the bodies too.
#[test]
fn a_cards_title_finds_every_cards_title() {
    let s = screens();
    assert_eq!(s.f.name(s.cards[0]), "Frame 1");
    assert_eq!(
        s.f.name(s.cards[1]),
        "Frame 2",
        "the fixture numbers its cards"
    );
    assert_eq!(s.f.name(s.titles[0]), "Text 1");
    assert_eq!(s.f.name(s.bodies[0]), "Text 2");
    assert_eq!(
        matching_layers(&s.f.doc, s.f.page, &[s.titles[0]]),
        s.titles
    );
    assert_eq!(
        matching_layers(&s.f.doc, s.f.page, &[s.bodies[3]]),
        s.bodies
    );
}

/// The header's title finds its counterpart on the other screen: the screens'
/// own names are not part of a place.
#[test]
fn the_same_layer_on_another_frame_matches_whatever_the_frame_is_called() {
    let s = screens();
    assert_eq!(
        matching_layers(&s.f.doc, s.f.page, &[s.headings[1]]),
        s.headings
    );
}

/// A sibling is never a copy: a card finds the card in the same position on the
/// other screen, not the card beside it.
#[test]
fn a_card_finds_its_counterpart_and_not_its_sibling() {
    let s = screens();
    assert_eq!(
        matching_layers(&s.f.doc, s.f.page, &[s.cards[0]]),
        vec![s.cards[0], s.cards[2]]
    );
}

/// A renamed layer leaves the set; the rest still match. Renaming a card's
/// *first* text would make its second the first "Text": generated names pair by
/// position.
#[test]
fn a_renamed_layer_does_not_match() {
    let mut s = screens();
    s.f.doc
        .apply(&Transaction(vec![Operation::SetName {
            id: s.bodies[3],
            name: "Price".into(),
        }]))
        .expect("rename");
    assert_eq!(
        matching_layers(&s.f.doc, s.f.page, &[s.bodies[0]]),
        s.bodies[..3].to_vec()
    );
    assert_eq!(
        matching_layers(&s.f.doc, s.f.page, &[s.titles[0]]),
        s.titles,
        "the titles are untouched"
    );
}

/// A top-level layer has no place, so it matches nothing — not even itself.
#[test]
fn a_top_level_layer_matches_nothing() {
    let s = screens();
    assert!(matching_layers(&s.f.doc, s.f.page, &[s.screen_a]).is_empty());
}

/// Several sources give the union of their sets, in document order.
#[test]
fn several_sources_union_their_sets() {
    let s = screens();
    let mut want = s.titles.clone();
    want.extend(&s.headings);
    let mut got = matching_layers(&s.f.doc, s.f.page, &[s.headings[0], s.titles[2]]);
    got.sort();
    want.sort();
    assert_eq!(got, want);
}

/// A repeat inside a repeat: in cards of two rows each, a row's label finds the
/// label of every row of every card, since each row is a copy too.
#[test]
fn a_repeat_inside_a_repeat_matches_at_both_levels() {
    let mut f = Fixture::new();
    let page = f.page;
    let screen = f.add(page, frame(), Some("Screen"));
    let list = f.add(screen, NodeKind::Group, Some("List"));
    let mut labels = Vec::new();
    for _ in 0..3 {
        let card = f.add(list, frame(), Some("Card"));
        for _ in 0..2 {
            let row = f.add(card, NodeKind::Group, Some("Row"));
            labels.push(f.add(row, text(), Some("Label")));
        }
    }
    assert_eq!(matching_layers(&f.doc, page, &[labels[2]]), labels);
}
