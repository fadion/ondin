//! Find and replace over text content (`ondin_core::find`): what a query finds
//! in which scope, what stands in a replace's way, and what a replace writes.

use ondin_core::find::{self, Query, Scope};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::{
    CharAttr, CharSpans, Document, IdSource, NodeId, NodeKind, Operation, Placement, TextSizing,
    TextStyle, Transaction,
};

fn style() -> TextStyle {
    TextStyle {
        font_family: "Inter".into(),
        font_size: 12.0,
        ..Default::default()
    }
}

fn text(content: &str, spans: CharSpans) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(style()),
        spans,
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn create(id: NodeId, parent: NodeId, kind: NodeKind, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind,
        transform: None,
        name: Some(name.into()),
    }
}

fn content(doc: &Document, id: NodeId) -> String {
    match doc.get(id).unwrap().kind() {
        NodeKind::Text { content, .. } => content.clone(),
        _ => unreachable!(),
    }
}

fn q(find: &str, replace: &str) -> Query {
    Query::new(find, replace, false, false, false, false).unwrap()
}

/// Two pages and a draft. On *Pricing*: a headline, a locked note, and a main
/// *Button* whose label reads *Team pricing*, with one instance of it. On
/// *Checkout*: a hidden hint. The draft holds one more *team*.
struct F {
    doc: Document,
    pricing: NodeId,
    checkout: NodeId,
    headline: NodeId,
    note: NodeId,
    label: NodeId,
    copy_label: NodeId,
    hint: NodeId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xF1);
    let pricing = ids.mint();
    let mut doc = Document::new(pricing);
    let [checkout, draft, headline, note, button, label, hint, scrap] = [(); 8].map(|_| ids.mint());
    let page = |id, index, name: &str, draft| Operation::AddPage {
        id,
        index,
        name: name.into(),
        ground: None,
        draft,
        mode: None,
    };
    doc.apply(&Transaction(vec![
        page(checkout, 1, "Checkout", false),
        page(draft, 2, "Scraps", true),
        create(
            button,
            pricing,
            NodeKind::Artboard {
                size: Size::new(100.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            "Button",
        ),
        create(
            label,
            button,
            text("Team pricing", Default::default()),
            "Label",
        ),
        Operation::SetComponent {
            id: button,
            component: true,
        },
        create(
            note,
            pricing,
            text("Talk to the team.", Default::default()),
            "Note",
        ),
        Operation::SetLocked {
            id: note,
            locked: true,
        },
        create(
            headline,
            pricing,
            text("Plans for every team", Default::default()),
            "Headline",
        ),
        create(
            hint,
            checkout,
            text("Invite your team later", Default::default()),
            "Hint",
        ),
        Operation::SetVisible {
            id: hint,
            visible: false,
        },
        create(scrap, draft, text("team", Default::default()), "Scrap"),
    ]))
    .unwrap();
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(button).unwrap(),
            parent: pricing,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).unwrap();
    let copy_label = doc.get(made[0]).unwrap().children()[0];
    F {
        doc,
        pricing,
        checkout,
        headline,
        note,
        label,
        copy_label,
        hint,
    }
}

/// **Every page but the drafts, in page then tree order, each match's state
/// told**: the locked note, the main's label with its one copy, the copy's own
/// label as inside an instance, and the hidden hint on the next page.
#[test]
fn all_pages_finds_each_match_with_what_stands_in_its_way() {
    let f = fixture();
    let hits = find::find(&f.doc, &Scope::Pages { here: f.pricing }, &q("team", ""));
    let nodes: Vec<NodeId> = hits.iter().map(|h| h.node).collect();
    assert_eq!(
        nodes,
        vec![f.copy_label, f.label, f.note, f.headline, f.hint],
        "tree order (top of the stack first), the draft left out"
    );
    let of = |id| hits.iter().find(|h| h.node == id).unwrap();
    assert!(of(f.note).locked && !of(f.note).writable(true));
    assert!(
        of(f.hint).hidden && of(f.hint).writable(false),
        "hidden is not locked"
    );
    assert_eq!(
        of(f.label).main.map(|(_, n)| n),
        Some(1),
        "one copy follows"
    );
    assert!(of(f.copy_label).instance.is_some() && !of(f.copy_label).writable(false));
    assert_eq!(of(f.headline).range, 16..20);
    assert_eq!(of(f.hint).page, f.checkout);

    let here = find::find(&f.doc, &Scope::Page(f.checkout), &q("team", ""));
    assert_eq!(here.len(), 1);
    let under = find::find(&f.doc, &Scope::Under(vec![f.headline]), &q("TEAM", ""));
    assert_eq!(under.len(), 1, "case ignored, the selection only");
    let case = Query::new("TEAM", "", true, false, false, false).unwrap();
    assert!(find::find(&f.doc, &Scope::Page(f.pricing), &case).is_empty());
}

/// **Whole words leaves *teams* alone**; without it the match is inside the word.
#[test]
fn whole_words_matches_only_between_boundaries() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetText {
            id: f.headline,
            content: "For small teams and a team".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]))
        .unwrap();
    let scope = Scope::Under(vec![f.headline]);
    assert_eq!(find::find(&f.doc, &scope, &q("team", "")).len(), 2);
    let words = Query::new("team", "", false, true, false, false).unwrap();
    let hits = find::find(&f.doc, &scope, &words);
    assert_eq!(
        hits.iter().map(|h| h.range.clone()).collect::<Vec<_>>(),
        vec![22..26]
    );
}

/// **A replace writes every writable match as one transaction, the locked note
/// and the copy left alone**, and says how many it wrote in how many layers.
///
/// flip: `replace` ignoring `writable` → fails at the counts, (5, 5).
#[test]
fn a_replace_skips_the_locked_and_the_copies_unless_asked() {
    let mut f = fixture();
    let query = q("team", "crew");
    let hits = find::find(&f.doc, &Scope::Pages { here: f.pricing }, &query);
    let done = find::replace(&f.doc, &hits, &query, false);
    assert_eq!((done.matches, done.layers), (3, 3));
    f.doc.apply(&done.tx).unwrap();
    assert_eq!(content(&f.doc, f.headline), "Plans for every crew");
    assert_eq!(content(&f.doc, f.hint), "Invite your crew later");
    assert_eq!(
        content(&f.doc, f.note),
        "Talk to the team.",
        "locked, never written"
    );
    assert_eq!(
        content(&f.doc, f.copy_label),
        "Team pricing",
        "a copy, not asked"
    );
    // The main's label as `keep_case` would not write it: as typed.
    assert_eq!(content(&f.doc, f.label), "crew pricing");

    let hits = find::find(&f.doc, &Scope::Page(f.pricing), &query);
    let done = find::replace(&f.doc, &hits, &query, true);
    f.doc.apply(&done.tx).unwrap();
    assert_eq!(
        content(&f.doc, f.copy_label),
        "crew pricing",
        "asked: its override"
    );
}

/// **Keep case carries each match's capitals**: *SIGN UP* → *LOG IN*, *Sign up*
/// → *Log in*.
#[test]
fn keep_case_gives_each_replacement_its_matchs_capitals() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetText {
            id: f.headline,
            content: "SIGN UP · Sign up".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]))
        .unwrap();
    let query = Query::new("sign up", "log in", false, false, false, true).unwrap();
    let hits = find::find(&f.doc, &Scope::Under(vec![f.headline]), &query);
    f.doc
        .apply(&find::replace(&f.doc, &hits, &query, false).tx)
        .unwrap();
    assert_eq!(content(&f.doc, f.headline), "LOG IN · Log in");
}

/// **A match across two runs becomes one run in its first character's style**:
/// *small te* bold, *ams* regular, replaced by *studios* — bold throughout, and
/// the text after it keeps its own; *small* regular, *teams* bold — regular.
///
/// flip: `Spans::replaced` as plain `edited` → fails at "regular throughout"
/// only: `edited` already lets a span starting at the match grow over it.
#[test]
fn a_replacement_takes_its_first_characters_style() {
    let mut f = fixture();
    let mut spans = CharSpans::default();
    spans.set(10..18, CharAttr::Weight(700), &style());
    f.doc
        .apply(&Transaction(vec![Operation::SetText {
            id: f.headline,
            content: "Built for small teams!".into(),
            spans,
            para_spans: Default::default(),
        }]))
        .unwrap();
    let query = q("small teams", "studios");
    let hits = find::find(&f.doc, &Scope::Under(vec![f.headline]), &query);
    f.doc
        .apply(&find::replace(&f.doc, &hits, &query, false).tx)
        .unwrap();
    assert_eq!(content(&f.doc, f.headline), "Built for studios!");
    let NodeKind::Text { spans, .. } = f.doc.get(f.headline).unwrap().kind() else {
        unreachable!()
    };
    let bold: Vec<_> = spans
        .as_slice()
        .iter()
        .filter(|s| s.attr == CharAttr::Weight(700))
        .map(|s| s.start..s.end)
        .collect();
    assert_eq!(bold, vec![10..17], "bold throughout, and nowhere else");

    // And the other way round: *small* regular, *teams* bold — all regular.
    let mut spans = CharSpans::default();
    spans.set(16..21, CharAttr::Weight(700), &style());
    f.doc
        .apply(&Transaction(vec![Operation::SetText {
            id: f.headline,
            content: "Built for small teams!".into(),
            spans,
            para_spans: Default::default(),
        }]))
        .unwrap();
    let hits = find::find(&f.doc, &Scope::Under(vec![f.headline]), &query);
    f.doc
        .apply(&find::replace(&f.doc, &hits, &query, false).tx)
        .unwrap();
    let NodeKind::Text { spans, .. } = f.doc.get(f.headline).unwrap().kind() else {
        unreachable!()
    };
    assert!(spans.is_empty(), "regular throughout: {spans:?}");
}

/// **A pattern's group goes where `[1]` stands**, and a stale hit — its text
/// changed since it was found — is not written.
#[test]
fn a_pattern_fills_its_groups_and_a_stale_hit_is_left_alone() {
    let mut f = fixture();
    let query = Query::new(r"every (\w+)", "each [1]", false, false, true, false).unwrap();
    let hits = find::find(&f.doc, &Scope::Under(vec![f.headline]), &query);
    assert_eq!(hits.len(), 1);
    f.doc
        .apply(&find::replace(&f.doc, &hits, &query, false).tx)
        .unwrap();
    assert_eq!(content(&f.doc, f.headline), "Plans for each team");
    let stale = find::replace(&f.doc, &hits, &query, false);
    assert_eq!(stale.matches, 0, "the text moved on");
    assert!(Query::new("(", "", false, false, true, false).is_err());
}
