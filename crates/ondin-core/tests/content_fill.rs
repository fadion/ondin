//! *Fill with content…* (`content_fill.rs`): what a layer's name and text say it
//! is, how a frame's values agree, that a draw is repeatable, that values fit
//! their box, and what the edit writes.

use ondin_core::content_fill::{
    self as fill, AvatarStyle, GROUNDS, Kind, Options, Plan, Value, kind_from_name, kind_from_text,
    locale_for, locales,
};
use ondin_core::kurbo::{Affine, BezPath, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::svg_in::ImageSink;
use ondin_core::verbs::{self, Ctx};
use ondin_core::{
    Document, IdSource, ImageEntry, ImageFormat, ImageId, ImageSource, NodeId, NodeKind, Operation,
    Resolved, TextSizing, TextStyle, Transaction,
};
use serde_json::json;

fn text(content: &str, sizing: TextSizing, locale: Option<&str>) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_size: 12.0,
            locale: locale.map(str::to_string),
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
    }
}

fn frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(300.0, 60.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn ellipse() -> NodeKind {
    NodeKind::Ellipse {
        size: Size::new(32.0, 32.0),
        arc: Default::default(),
    }
}

struct Fixture {
    doc: Document,
    ids: IdSource,
    page: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut ids = IdSource::new(0xF111);
        let page = ids.mint();
        Self {
            doc: Document::new(page),
            ids,
            page,
        }
    }

    fn commit(&mut self, ops: Vec<Operation>) {
        self.doc.apply(&Transaction(ops)).expect("fixture edit");
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind, name: &str) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::IDENTITY),
            name: Some(name.into()),
        }]);
        id
    }

    /// The artboard's team list: a heading and three rows, each an avatar, a
    /// name, an email and a role. Answers the list and its rows.
    fn team(&mut self) -> (NodeId, Vec<NodeId>) {
        let list = self.add(self.page, frame(), "Team list");
        self.add(list, text("Our team", TextSizing::Auto, None), "Heading");
        let rows = (1..=3)
            .map(|i| {
                let row = self.add(list, frame(), &format!("Row {i}"));
                self.add(row, ellipse(), "Avatar");
                self.add(row, text("Name", TextSizing::Auto, None), "Name");
                self.add(
                    row,
                    text("email@example.com", TextSizing::Auto, None),
                    "Email",
                );
                self.add(row, text("Role", TextSizing::Auto, None), "Role");
                row
            })
            .collect();
        (list, rows)
    }

    fn plan(&self, roots: &[NodeId], opts: &Options) -> Plan {
        fill::plan(&self.doc, roots, opts)
    }

    fn text_of(plan: &Plan, node: NodeId) -> String {
        match &plan.rows.iter().find(|r| r.node == node).unwrap().value {
            Value::Text(t) => t.clone(),
            other => panic!("{other:?} is not text"),
        }
    }

    fn child(&self, parent: NodeId, name: &str) -> NodeId {
        *self
            .doc
            .get(parent)
            .unwrap()
            .children()
            .iter()
            .find(|c| self.doc.get(**c).unwrap().name() == name)
            .unwrap()
    }
}

/// Every shipped locale reads with the lists and formats a fill draws from, and
/// a text's Language finds its locale by tag, then by language.
#[test]
fn every_locale_has_its_tables_and_is_found_by_language() {
    assert_eq!(locales().len(), 8);
    for l in locales() {
        for kind in Kind::ALL.into_iter().filter(|k| k.is_text()) {
            assert!(
                !fill::sample(kind, &l.tag).trim().is_empty(),
                "{} draws a {}",
                l.tag,
                kind.label()
            );
        }
    }
    assert_eq!(locale_for("de-AT").unwrap().tag, "de");
    assert_eq!(locale_for("en").unwrap().tag, "en-US");
    assert_eq!(locale_for("en-GB").unwrap().tag, "en-GB");
    assert_eq!(locale_for("pt").unwrap().tag, "pt-BR");
    assert!(locale_for("ja").is_none());
}

/// A name's phrase wins over a word inside it (*Job title* is a job, *Company
/// name* a company), and a text reads by its shape when the name says nothing.
#[test]
fn a_layer_is_read_by_its_name_then_its_text() {
    assert_eq!(kind_from_name("Name"), Some(Kind::FullName));
    assert_eq!(kind_from_name("Full name 2"), Some(Kind::FullName));
    assert_eq!(kind_from_name("nombre"), Some(Kind::FullName));
    assert_eq!(kind_from_name("Job title"), Some(Kind::JobTitle));
    assert_eq!(kind_from_name("Company name"), Some(Kind::Company));
    assert_eq!(kind_from_name("Vorname"), Some(Kind::FirstName));
    assert_eq!(kind_from_name("E-mail"), Some(Kind::Email));
    assert_eq!(kind_from_name("Card title"), Some(Kind::Headline));
    assert_eq!(kind_from_name("Heading"), None, "a list's heading is left");
    assert_eq!(kind_from_name("Text 12"), None);
    assert_eq!(kind_from_text("maya@northwind.io"), Some(Kind::Email));
    assert_eq!(kind_from_text("$12.00"), Some(Kind::Price));
    assert_eq!(kind_from_text("1.249,00 €"), Some(Kind::Price));
    assert_eq!(kind_from_text("(415) 555-0132"), Some(Kind::Phone));
    assert_eq!(kind_from_text("42%"), Some(Kind::Percent));
    assert_eq!(kind_from_text("9:30 AM"), Some(Kind::Time));
    assert_eq!(kind_from_text("08.11.2026"), Some(Kind::Date));
    assert_eq!(kind_from_text("Nov 8, 2026"), Some(Kind::Date));
    assert_eq!(kind_from_text("1,249"), Some(Kind::Number));
    assert_eq!(kind_from_text("Lorem ipsum dolor"), Some(Kind::Paragraph));
    assert_eq!(kind_from_text("Our team"), None);
}

/// Over the team list each row is a record: its email is built from its own
/// name, its avatar's initials are its name's, the rows differ, and the heading
/// is left as it is. The count names the rows after the frames.
/// flip: `record_roots` not taking repeating children → fails at "three rows".
#[test]
fn each_row_of_a_list_is_a_record_whose_values_agree() {
    let mut f = Fixture::new();
    let (list, rows) = f.team();
    let plan = f.plan(&[list], &Options::default());
    assert_eq!(plan.filled_records().len(), 3, "three rows");
    assert_eq!(plan.record_noun(&f.doc), "row");
    assert_eq!(plan.rows.len(), 13, "a heading and four layers a row");
    assert_eq!(plan.left(), 1, "the heading");
    let mut names = Vec::new();
    for row in rows {
        let name = Fixture::text_of(&plan, f.child(row, "Name"));
        let email = Fixture::text_of(&plan, f.child(row, "Email"));
        let given = name.split(' ').next().unwrap().to_lowercase();
        let first = given.chars().next().unwrap();
        assert!(
            email.starts_with(&given) || email.starts_with(&format!("{first}.")),
            "{email} is built from {name}"
        );
        let avatar = plan
            .rows
            .iter()
            .find(|r| r.node == f.child(row, "Avatar"))
            .unwrap();
        match &avatar.value {
            Value::Avatar(a) => assert_eq!(a.name, name, "the avatar is the row's person"),
            other => panic!("{other:?}"),
        }
        names.push(name);
    }
    names.dedup();
    assert_eq!(names.len(), 3, "a different person a row: {names:?}");
}

/// The same seed fills the same values; Shuffle's new seed fills others; a
/// row's redraw changes its record's person, name and email together, and no
/// other row.
#[test]
fn a_draw_repeats_and_a_redraw_moves_only_its_record() {
    let mut f = Fixture::new();
    let (list, rows) = f.team();
    let opts = Options::default();
    let a = f.plan(&[list], &opts);
    let b = f.plan(&[list], &opts);
    let values = |p: &Plan| p.rows.iter().map(|r| r.value.clone()).collect::<Vec<_>>();
    assert_eq!(values(&a), values(&b), "one seed, one fill");
    let shuffled = f.plan(
        &[list],
        &Options {
            seed: 9,
            ..Options::default()
        },
    );
    assert_ne!(values(&a), values(&shuffled), "another seed, another fill");

    let name = f.child(rows[0], "Name");
    let mut redraw = Options::default();
    redraw.redraws.insert(name, 1);
    let c = f.plan(&[list], &redraw);
    let changed: Vec<NodeId> = a
        .rows
        .iter()
        .zip(&c.rows)
        .filter(|(x, y)| x.value != y.value)
        .map(|(x, _)| x.node)
        .collect();
    assert!(changed.contains(&name), "the row redrawn changes");
    assert!(
        changed.contains(&f.child(rows[0], "Email")),
        "its email follows its name"
    );
    assert!(
        changed
            .iter()
            .all(|id| f.doc.get(*id).unwrap().parent() == Some(rows[0])),
        "nothing outside its row moves: {changed:?}"
    );
}

/// With records off, layers that share a frame no longer agree: each is drawn
/// on its own.
#[test]
fn records_off_draws_every_layer_on_its_own() {
    let mut f = Fixture::new();
    let (list, _) = f.team();
    let plan = f.plan(
        &[list],
        &Options {
            records: false,
            ..Options::default()
        },
    );
    assert!(plan.filled_records().is_empty());
    assert_eq!(plan.records.len(), plan.rows.len());
}

/// A fixed single line takes a value that fits its width, or after its draws
/// keeps an overflowing one marked as such; a fixed box fills sentence by
/// sentence and fits.
#[test]
fn values_fit_their_box() {
    let mut f = Fixture::new();
    let roomy = f.add(
        f.page,
        text("Name", TextSizing::Fixed(Size::new(400.0, 16.0)), None),
        "Name",
    );
    let tiny = f.add(
        f.page,
        text("Name", TextSizing::Fixed(Size::new(4.0, 16.0)), None),
        "Name",
    );
    let body = f.add(
        f.page,
        text("Body", TextSizing::Fixed(Size::new(240.0, 160.0)), None),
        "Description",
    );
    let plan = f.plan(&[roomy, tiny, body], &Options::default());
    let row = |id| plan.rows.iter().find(|r| r.node == id).unwrap();
    assert!(row(roomy).fits);
    assert!(!row(tiny).fits, "nothing fits four points");
    assert!(matches!(&row(tiny).value, Value::Text(t) if !t.is_empty()));
    assert!(row(body).fits);
    let para = Fixture::text_of(&plan, body);
    assert!(
        para.matches(". ").count() >= 2,
        "a 240 × 160 box takes several sentences: {para}"
    );

    let line = f.add(
        f.page,
        text("x", TextSizing::Fixed(Size::new(900.0, 16.0)), None),
        "Address",
    );
    let plan = f.plan(&[line], &Options::default());
    let address = Fixture::text_of(&plan, line);
    assert!(
        !address.contains('\n') && address.contains(", "),
        "one line reads with commas: {address}"
    );
}

/// A Language the layers agree on picks the locale, and that locale's formats
/// write the values.
#[test]
fn the_layers_language_picks_the_locale() {
    let mut f = Fixture::new();
    let price = f.add(
        f.page,
        text("1,00 €", TextSizing::Auto, Some("de")),
        "Preis",
    );
    let date = f.add(f.page, text("x", TextSizing::Auto, Some("de-DE")), "Datum");
    assert_eq!(
        fill::agreed_locale(&f.doc, &[price, date]).unwrap().tag,
        "de"
    );
    let plain = f.add(f.page, text("x", TextSizing::Auto, None), "Heading");
    assert_eq!(
        fill::agreed_locale(&f.doc, &[price, plain]).unwrap().tag,
        "de",
        "a text without a Language says nothing"
    );
    let other = f.add(f.page, text("x", TextSizing::Auto, Some("fr")), "Datum");
    assert!(fill::agreed_locale(&f.doc, &[price, other]).is_none());
    let plan = f.plan(
        &[price, date],
        &Options {
            locale: "de".into(),
            ..Options::default()
        },
    );
    assert!(Fixture::text_of(&plan, price).ends_with("\u{A0}€"));
    assert_eq!(Fixture::text_of(&plan, date).matches('.').count(), 2);
}

/// Locked layers are counted and skipped; a layer the Content row or its own
/// kind names is filled whatever its name says.
#[test]
fn locked_layers_are_skipped_and_chosen_kinds_win() {
    let mut f = Fixture::new();
    let a = f.add(f.page, text("x", TextSizing::Auto, None), "Text");
    let b = f.add(f.page, text("x", TextSizing::Auto, None), "Name");
    f.commit(vec![Operation::SetLocked {
        id: b,
        locked: true,
    }]);
    let plan = f.plan(&[a, b], &Options::default());
    assert_eq!(plan.locked, 1);
    assert_eq!(plan.rows.len(), 1);
    assert_eq!(plan.rows[0].kind, None, "*Text* reads as nothing");
    let all = f.plan(
        &[a],
        &Options {
            content: Some(Kind::City),
            ..Options::default()
        },
    );
    assert_eq!(all.rows[0].kind, Some(Kind::City));
    let mut own = Options {
        content: Some(Kind::City),
        ..Options::default()
    };
    own.kinds.insert(a, Some(Kind::Email));
    assert_eq!(f.plan(&[a], &own).rows[0].kind, Some(Kind::Email));
}

/// Hashes nothing, decodes nothing: names each drawing by its size and paths.
struct Painter(usize);

impl ImageSink for Painter {
    fn place(&mut self, _: Vec<u8>, _: ImageFormat) -> Option<(ImageId, ImageEntry)> {
        None
    }

    fn draw(&mut self, w: u32, h: u32, art: &[(BezPath, Color)]) -> Option<(ImageId, ImageEntry)> {
        self.0 += 1;
        Some((
            ImageId(format!("avatar-{w}x{h}-{}", art.len())),
            ImageEntry {
                source: ImageSource::Embedded(vec![0u8; 4].into()),
                format: ImageFormat::Png,
                width: w,
                height: h,
            },
        ))
    }
}

struct NoPainter;

impl ImageSink for NoPainter {
    fn place(&mut self, _: Vec<u8>, _: ImageFormat) -> Option<(ImageId, ImageEntry)> {
        None
    }
}

/// The edit sets each text once, and gives an avatar shape one picture fill
/// drawn at twice its size; a sink that cannot draw leaves avatars out.
#[test]
fn the_edit_writes_texts_and_draws_avatars_at_twice_their_size() {
    let mut f = Fixture::new();
    let (list, _) = f.team();
    let plan = f.plan(&[list], &Options::default());
    let mut painter = Painter(0);
    let tx = plan.transaction(&f.doc, Some(&mut painter));
    let texts =
        tx.0.iter()
            .filter(|op| matches!(op, Operation::SetText { .. }))
            .count();
    assert_eq!(texts, 9, "three texts a row");
    assert_eq!(painter.0, 3, "an avatar a row");
    assert!(tx.0.iter().any(|op| matches!(
        op,
        Operation::AddImage { entry, .. } if entry.width == 64 && entry.height == 64
    )));
    f.doc.apply(&tx).expect("the fill applies");

    let plan = f.plan(&[list], &Options::default());
    let tx = plan.transaction(&f.doc, Some(&mut NoPainter));
    assert!(
        !tx.0
            .iter()
            .any(|op| matches!(op, Operation::SetFills { .. })),
        "nothing to draw with"
    );
    let off = f.plan(
        &[list],
        &Options {
            avatars: None,
            ..Options::default()
        },
    );
    assert_eq!(off.rows.len(), 10, "with avatars off their rows drop out");
}

/// Every avatar ground holds white initials at 4.5:1 or better, and the two
/// styles draw the ground plus initials or three shapes.
#[test]
fn avatar_grounds_hold_white_and_both_styles_draw() {
    let lum = |c: Color| {
        let [r, g, b, _] = c.components;
        let lin = |v: f32| {
            let v = f64::from(v);
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
    };
    for g in GROUNDS {
        let ratio = 1.05 / (lum(g) + 0.05);
        assert!(ratio >= 4.5, "{g:?} is {ratio:.2}:1 against white");
    }
    let avatar = |style| fill::Avatar {
        name: "Maya Okafor".into(),
        initials: "MO".into(),
        style,
    };
    assert_eq!(avatar(AvatarStyle::Initials).art(64.0, 64.0).len(), 2);
    assert_eq!(avatar(AvatarStyle::Pattern).art(64.0, 64.0).len(), 4);
    assert_eq!(
        avatar(AvatarStyle::Initials).ground(),
        avatar(AvatarStyle::Pattern).ground(),
        "one person, one ground"
    );
}

/// `fill_content` fills the layers named, in the locale given.
#[test]
fn the_verb_fills_in_the_locale_given() {
    let mut f = Fixture::new();
    let (list, rows) = f.team();
    let res = Resolved::rebuild(&f.doc);
    let done = verbs::call(
        &mut Ctx {
            doc: &f.doc,
            res: &res,
            ids: &mut f.ids,
            images: None,
            contrast: None,
        },
        "fill_content",
        &json!({ "nodes": [list.to_wire()], "locale": "nl", "seed": 3 }),
    )
    .expect("fill_content runs");
    f.doc.apply(&done.tx).expect("it applies");
    let email = f.child(rows[0], "Email");
    let NodeKind::Text { content, .. } = f.doc.get(email).unwrap().kind() else {
        unreachable!()
    };
    assert!(content.ends_with(".nl"), "a Dutch domain: {content}");
}
