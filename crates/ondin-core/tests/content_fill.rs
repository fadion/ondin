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
        fill::plan(&self.doc, &Resolved::rebuild(&self.doc), roots, opts)
    }

    /// `plan`'s edit, with avatars drawn through `pictures`.
    fn transaction(&self, plan: &Plan, pictures: Option<&mut dyn ImageSink>) -> Transaction {
        plan.transaction(&self.doc, &Resolved::rebuild(&self.doc), pictures)
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

/// A decimal and a count grouped in thousands read as numbers, not as a time or
/// a phone number; a clock reads as a time only with hours and minutes a clock
/// shows.
///
/// flip: `kind_from_text` taking `PHONE` without the `GROUPED` check → fails at
/// "1.250.000".
#[test]
fn a_decimal_or_a_grouped_count_reads_as_a_number() {
    for t in ["25.99", "99.99", "12.50", "4.75", "1.250.000", "1 250 000"] {
        assert_eq!(kind_from_text(t), Some(Kind::Number), "{t}");
    }
    for t in ["24:30", "9:75", "13:00 PM"] {
        assert_ne!(kind_from_text(t), Some(Kind::Time), "{t}");
    }
    for t in ["23:59", "7.15 p.m.", "12:00 PM", "08:05"] {
        assert_eq!(kind_from_text(t), Some(Kind::Time), "{t}");
    }
    for t in ["612 345 678", "06 39 98 12 34", "+49 30 901820"] {
        assert_eq!(kind_from_text(t), Some(Kind::Phone), "{t}");
    }
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
    let tx = f.transaction(&plan, Some(&mut painter));
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
    let tx = f.transaction(&plan, Some(&mut NoPainter));
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

impl Fixture {
    /// A frame of `n` rows, each a frame holding a text layer per field, named
    /// and reading as the field. Answers the list and its rows.
    fn list(&mut self, n: usize, fields: &[&str], locale: Option<&str>) -> (NodeId, Vec<NodeId>) {
        let list = self.add(self.page, frame(), "List");
        let rows = (1..=n)
            .map(|i| {
                let row = self.add(list, frame(), &format!("Row {i}"));
                for field in fields {
                    self.add(row, text(field, TextSizing::Auto, locale), field);
                }
                row
            })
            .collect();
        (list, rows)
    }

    /// Each row's value of its layer named `field`.
    fn column(&self, plan: &Plan, rows: &[NodeId], field: &str) -> Vec<String> {
        rows.iter()
            .map(|r| Self::text_of(plan, self.child(*r, field)))
            .collect()
    }
}

/// The entries of a table's `[name]` section.
fn section<'a>(source: &'a str, name: &str) -> Vec<&'a str> {
    let head = format!("[{name}]");
    source
        .lines()
        .map(str::trim_end)
        .skip_while(|l| *l != head)
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

fn opts(locale: &str) -> Options {
    Options {
        locale: locale.into(),
        ..Options::default()
    }
}

/// Two cards filled one call at a time with one seed get different people: a
/// record is seeded by its frame, not by being the first in the fill.
/// flip: seeding a record by its index → fails at "another card".
#[test]
fn cards_filled_one_at_a_time_differ() {
    let mut f = Fixture::new();
    let a = f.add(f.page, frame(), "Card");
    let b = f.add(f.page, frame(), "Card");
    let name_a = f.add(a, text("Name", TextSizing::Auto, None), "Name");
    let name_b = f.add(b, text("Name", TextSizing::Auto, None), "Name");
    let first = Fixture::text_of(&f.plan(&[a], &Options::default()), name_a);
    let second = Fixture::text_of(&f.plan(&[b], &Options::default()), name_b);
    assert_ne!(first, second, "another card, another person");
}

/// Down a long list no given name, job or city repeats while the tables have
/// others, and a redraw still moves only its own row.
/// flip: `Rng::pick_fresh` returning its first draw whatever is taken → fails
/// at "no name twice".
#[test]
fn a_long_list_repeats_no_name_job_or_city() {
    let mut f = Fixture::new();
    let (list, rows) = f.list(20, &["Name", "Role", "City"], None);
    let plan = f.plan(&[list], &Options::default());
    let distinct = |xs: &[String]| xs.iter().collect::<std::collections::HashSet<_>>().len();
    let names = f.column(&plan, &rows, "Name");
    let given: Vec<String> = names
        .iter()
        .map(|n| n.split(' ').next().unwrap().to_string())
        .collect();
    assert_eq!(distinct(&given), 20, "no name twice: {given:?}");
    let jobs = f.column(&plan, &rows, "Role");
    assert_eq!(distinct(&jobs), 20, "no job twice: {jobs:?}");
    let cities = f.column(&plan, &rows, "City");
    assert_eq!(distinct(&cities), 20, "no city twice: {cities:?}");

    let mut redraw = Options::default();
    redraw.redraws.insert(f.child(rows[5], "Name"), 1);
    let again = f.plan(&[list], &redraw);
    for (i, row) in rows.iter().enumerate() {
        let before = Fixture::text_of(&plan, f.child(*row, "Name"));
        let after = Fixture::text_of(&again, f.child(*row, "Name"));
        assert_eq!(before == after, i != 5, "row {i}: {before} → {after}");
    }
    let redrawn = f.column(&again, &rows, "Name");
    assert_eq!(
        distinct(&redrawn),
        20,
        "the redrawn row takes no one else's name"
    );
}

/// In German a person's job takes the form that agrees with their name.
#[test]
fn a_job_agrees_with_its_person() {
    let de = include_str!("../assets/content/de.txt");
    let women: std::collections::HashSet<&str> = section(de, "given-f").into_iter().collect();
    let mut form_of = std::collections::HashMap::new();
    for pair in section(de, "jobs") {
        if let Some((m, w)) = pair.split_once('|') {
            form_of.insert(m, false);
            form_of.insert(w, true);
        }
    }
    let mut f = Fixture::new();
    let (list, rows) = f.list(30, &["Name", "Role"], None);
    let plan = f.plan(&[list], &opts("de"));
    let mut gendered = 0;
    for (name, job) in f
        .column(&plan, &rows, "Name")
        .iter()
        .zip(f.column(&plan, &rows, "Role"))
    {
        let woman = women.contains(name.split(' ').next().unwrap());
        if let Some(feminine) = form_of.get(job.as_str()) {
            gendered += 1;
            assert_eq!(*feminine, woman, "{name}, {job}");
        }
    }
    assert!(gendered >= 10, "the list drew gendered jobs: {gendered}");
}

/// The most specific phrase wins, a short label is not body copy, and a
/// shape's name makes it an avatar only when it is about a picture of someone.
#[test]
fn names_read_most_specific_first() {
    assert_eq!(kind_from_name("Street address"), Some(Kind::Street));
    assert_eq!(kind_from_name("Product price"), Some(Kind::Price));
    assert_eq!(kind_from_name("Product name"), Some(Kind::Product));
    assert_eq!(kind_from_name("Team member"), Some(Kind::FullName));
    assert_eq!(kind_from_name("Team"), Some(Kind::Department));
    assert_eq!(kind_from_name("Updated time"), Some(Kind::Time));
    assert_eq!(kind_from_name("Bio"), Some(Kind::Bio));
    assert_eq!(kind_from_name("Testimonial 2"), Some(Kind::Quote));
    assert_eq!(kind_from_name("Table cell"), None);
    assert_eq!(fill::kind_of_text_layer("About", "About"), None);
    assert_eq!(fill::kind_of_text_layer("Copy link", "Copy link"), None);
    assert_eq!(
        fill::kind_of_text_layer("Description", "A few words about it"),
        Some(Kind::Paragraph)
    );
    assert_eq!(
        fill::kind_of_text_layer("Name", "Name"),
        Some(Kind::FullName),
        "a placeholder named by its text is a slot until a value shows it is a label"
    );
    for name in ["Avatar", "User avatar", "Photo", "Profile picture", "Pfp 2"] {
        assert!(fill::is_avatar_name(name), "{name}");
    }
    for name in ["Profile header bg", "Product photo", "Picture frame border"] {
        assert!(!fill::is_avatar_name(name), "{name}");
    }
}

/// A label named by its text is left where another layer shows its kind: a
/// field's *Email* beside its value, a table's *Name* header over its rows.
/// flip: dropping the `labels` pass in `layout` → fails at "the label".
#[test]
fn a_label_over_its_value_is_left() {
    let mut f = Fixture::new();
    let field = f.add(f.page, frame(), "Field");
    let label = f.add(field, text("Email", TextSizing::Auto, None), "Email");
    let value = f.add(
        field,
        text("jane@acme.com", TextSizing::Auto, None),
        "jane@acme.com",
    );
    let plan = f.plan(&[field], &Options::default());
    let row = |p: &Plan, id| p.rows.iter().find(|r| r.node == id).unwrap().value.clone();
    assert_eq!(row(&plan, label), Value::Kept, "the label");
    assert!(Fixture::text_of(&plan, value).contains('@'));

    let (table, rows) = f.list(3, &["Name"], None);
    let header = f.add(table, text("Name", TextSizing::Auto, None), "Name");
    let plan = f.plan(&[table], &Options::default());
    assert_eq!(row(&plan, header), Value::Kept, "the header");
    assert!(
        f.column(&plan, &rows, "Name")
            .iter()
            .all(|n| n.contains(' '))
    );
}

/// Phone numbers come from the blocks reserved for fiction where a country has
/// them, and a UK postcode's district never reads 0.
#[test]
fn phone_numbers_are_reserved_for_fiction() {
    let blocks: [(&str, &[&str], &[&str]); 4] = [
        ("en-US", &["555-01"], &["555-01"]),
        (
            "en-GB",
            &[
                "020 7946 0",
                " 496 0",
                "0191 498 0",
                "029 2018 0",
                "028 9649 6",
                "01632 960",
            ],
            &["07700 900"],
        ),
        (
            "fr",
            &["01 99 00", "02 61 91", "03 53 01", "04 65 71", "05 36 49"],
            &["06 39 98"],
        ),
        (
            "de",
            &[
                "030 23125",
                "069 90009",
                "040 66969",
                "0221 4710",
                "089 99998",
            ],
            &["0171 39200", "0176 040690"],
        ),
    ];
    for (tag, phone, mobile) in blocks {
        let mut f = Fixture::new();
        let (list, rows) = f.list(24, &["Phone", "Mobile", "Postcode"], None);
        let plan = f.plan(&[list], &opts(tag));
        for p in f.column(&plan, &rows, "Phone") {
            // A German number outside the five cities starts with 0 after its
            // area code, which is never assigned.
            let unassigned = tag == "de" && p.split(' ').nth(1).is_some_and(|s| s.starts_with('0'));
            assert!(
                unassigned || phone.iter().any(|b| p.contains(b)),
                "{tag} phone {p}"
            );
        }
        for m in f.column(&plan, &rows, "Mobile") {
            assert!(
                mobile.iter().any(|b| m.starts_with(b) || m.contains(b)),
                "{tag} mobile {m}"
            );
        }
        if tag == "en-GB" {
            for pc in f.column(&plan, &rows, "Postcode") {
                let district = pc.split(' ').next().unwrap();
                assert!(!district.ends_with('0'), "{pc}");
            }
        }
    }
}

/// With example emails on, every address is at example.com.
/// flip: `Drawn::domain` ignoring `example_emails` → fails at the assertion.
#[test]
fn example_emails_reach_no_one() {
    let mut f = Fixture::new();
    let (list, rows) = f.list(5, &["Name", "Email", "Company"], None);
    let plan = f.plan(
        &[list],
        &Options {
            example_emails: true,
            ..Options::default()
        },
    );
    for email in f.column(&plan, &rows, "Email") {
        assert!(email.ends_with("@example.com"), "{email}");
    }
}

/// The outline counts the rows, changes and records the plan does, undrawn.
#[test]
fn the_outline_counts_as_the_plan_does() {
    let mut f = Fixture::new();
    let (list, _) = f.team();
    let plan = f.plan(&[list], &Options::default());
    let outline = fill::outline(&f.doc, &[list], &Options::default());
    assert_eq!(outline.rows.len(), plan.rows.len());
    assert_eq!(outline.changes(), plan.changes());
    assert_eq!(outline.filled_records(), plan.filled_records());
}

/// In every locale, every kind's value is whole: no slot left unfilled and no
/// gendered pair left unsplit. Numbers run from one digit to four, and a
/// birthday is decades back.
#[test]
fn every_value_is_whole() {
    let fields: Vec<&str> = Kind::ALL
        .into_iter()
        .filter(|k| k.is_text())
        .map(Kind::label)
        .collect();
    for l in locales() {
        let mut f = Fixture::new();
        let (list, rows) = f.list(6, &fields, None);
        let mut o = opts(&l.tag);
        for row in &rows {
            for (field, kind) in fields
                .iter()
                .zip(Kind::ALL.into_iter().filter(|k| k.is_text()))
            {
                o.kinds.insert(f.child(*row, field), Some(kind));
            }
        }
        let plan = f.plan(&[list], &o);
        for r in &plan.rows {
            let Value::Text(t) = &r.value else {
                panic!("{:?} drew nothing in {}", r.kind, l.tag)
            };
            assert!(
                !t.is_empty() && !t.contains(['{', '}', '|']),
                "{} {:?}: {t}",
                l.tag,
                r.kind
            );
        }
    }

    let mut f = Fixture::new();
    let (list, rows) = f.list(40, &["Count", "Birthday"], None);
    let plan = f.plan(&[list], &Options::default());
    let counts = f.column(&plan, &rows, "Count");
    assert!(
        counts.iter().any(|c| c.len() <= 2),
        "short counts: {counts:?}"
    );
    for day in f.column(&plan, &rows, "Birthday") {
        let year: u32 = day.rsplit(' ').next().unwrap().parse().unwrap();
        assert!(year <= 2004, "{day}");
    }
}

/// A filled text keeps its first character's style and its first paragraph's:
/// a bold heading over a bulleted line stays a bold heading, not a bullet.
///
/// flip: `Plan::transaction` re-basing the paragraph spans with `edited` → fails
/// at "no bullet".
#[test]
fn a_filled_text_keeps_its_first_character_and_paragraph() {
    let mut f = Fixture::new();
    let NodeKind::Text {
        style, paragraph, ..
    } = text("x", TextSizing::Auto, None)
    else {
        unreachable!()
    };
    let content = "Features\nFast";
    let mut spans = ondin_core::CharSpans::default();
    spans.set(0..8, ondin_core::CharAttr::Weight(700), &style);
    let mut para_spans = ondin_core::ParaSpans::default();
    para_spans.set(
        9..13,
        ondin_core::ParaAttr::Marker(Some(ondin_core::ListMarker::Disc)),
        &paragraph,
    );
    let heading = f.add(f.page, text(content, TextSizing::Auto, None), "Headline");
    f.commit(vec![Operation::SetText {
        id: heading,
        content: content.into(),
        spans,
        para_spans,
    }]);
    let plan = f.plan(&[heading], &Options::default());
    let tx = f.transaction(&plan, None);
    f.doc.apply(&tx).expect("the fill applies");
    let NodeKind::Text {
        content,
        style,
        spans,
        para_spans,
        paragraph,
        ..
    } = f.doc.get(heading).unwrap().kind()
    else {
        unreachable!()
    };
    assert_ne!(content, "Features\nFast", "fixture: filled");
    let len = content.len();
    assert!(
        spans.runs(style, len).iter().all(|r| r.1.weight == 700),
        "bold throughout"
    );
    assert!(
        para_spans
            .runs(paragraph, len)
            .iter()
            .all(|r| r.1.marker.is_none()),
        "no bullet"
    );
}

/// An avatar drawn over a credited photo takes the photo's credit with it, as a
/// replace does.
///
/// flip: `Plan::transaction` not appending `credit::dropped_by_replace` → fails
/// at "the credit goes".
#[test]
fn an_avatar_over_a_credited_photo_drops_the_credit() {
    let mut f = Fixture::new();
    let shape = f.add(f.page, ellipse(), "Avatar");
    let photo = ImageId("photo".into());
    f.commit(vec![
        Operation::AddImage {
            id: photo.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![0u8; 4].into()),
                format: ImageFormat::Png,
                width: 4,
                height: 4,
            },
        },
        Operation::SetFills {
            id: shape,
            fills: ondin_core::keyed_by_position([ondin_core::Fill {
                brush: ondin_core::image_brush(photo).into(),
                visible: true,
            }]),
        },
        Operation::SetCredit {
            id: shape,
            credit: Some(Box::new(ondin_core::Credit {
                kind: ondin_core::CreditKind::Photo,
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
            })),
        },
    ]);
    let plan = f.plan(&[shape], &Options::default());
    let tx = f.transaction(&plan, Some(&mut Painter(0)));
    f.doc.apply(&tx).expect("the fill applies");
    assert!(
        f.doc.get(shape).unwrap().paint().image_fill().is_some(),
        "fixture: an avatar drawn"
    );
    assert_eq!(f.doc.get(shape).unwrap().credit(), None, "the credit goes");
}

/// Hidden layers, a text bound to a variable and a text under a locked frame are
/// not filled, the last counted as locked; a shape showing a picture is listed
/// and kept, and a frame holding layers is no avatar's place.
///
/// flip: `gather` without its `shown` check → fails at "nothing hidden".
#[test]
fn hidden_bound_and_locked_layers_are_passed_over() {
    let mut f = Fixture::new();
    let hidden = f.add(f.page, frame(), "Card");
    f.add(hidden, text("Name", TextSizing::Auto, None), "Name");
    f.commit(vec![Operation::SetVisible {
        id: hidden,
        visible: false,
    }]);
    assert!(
        f.plan(&[hidden], &Options::default()).rows.is_empty(),
        "nothing hidden"
    );

    let bound = f.add(f.page, text("Name", TextSizing::Auto, None), "Name");
    let (ops, var) = ondin_core::variable::add_variable_ops(
        f.doc.variables(),
        &mut f.ids,
        ondin_core::variable::CollectionId::FIRST,
        ondin_core::VarKind::String,
        Some("label"),
        None,
    )
    .unwrap();
    f.commit(ops);
    let mut vars = ondin_core::TextVars::default();
    vars[ondin_core::variable::TEXT_CONTENT] = Some(var);
    let bind = ondin_core::bind_text_ops(&f.doc, bound, vars);
    f.commit(bind);
    assert!(
        f.plan(&[bound], &Options::default()).rows.is_empty(),
        "a bound text is its variable's"
    );

    let locked = f.add(f.page, frame(), "Card");
    f.add(locked, text("Name", TextSizing::Auto, None), "Name");
    f.commit(vec![Operation::SetLocked {
        id: locked,
        locked: true,
    }]);
    let plan = f.plan(&[locked], &Options::default());
    assert!(plan.rows.is_empty());
    assert_eq!(plan.locked, 1, "the text under the locked frame");

    let pictured = f.add(f.page, ellipse(), "Shape");
    let photo = ImageId("photo".into());
    f.commit(vec![
        Operation::AddImage {
            id: photo.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(vec![0u8; 4].into()),
                format: ImageFormat::Png,
                width: 4,
                height: 4,
            },
        },
        Operation::SetFills {
            id: pictured,
            fills: ondin_core::keyed_by_position([ondin_core::Fill {
                brush: ondin_core::image_brush(photo).into(),
                visible: true,
            }]),
        },
    ]);
    let holder = f.add(f.page, frame(), "Avatar");
    f.add(holder, text("Hi", TextSizing::Auto, None), "Greeting");
    let plan = f.plan(&[pictured, holder], &Options::default());
    let row = plan
        .rows
        .iter()
        .find(|r| r.node == pictured)
        .expect("listed");
    assert_eq!(
        row.value,
        Value::Kept,
        "a picture not named an avatar is kept"
    );
    assert!(
        plan.rows.iter().all(|r| r.node != holder),
        "a frame holding layers is no avatar"
    );
}

/// Three avatars drawn alike add their picture once.
///
/// flip: `Plan::transaction` adding a picture without its `added` set → fails at
/// "one picture" (the table refuses a second `AddImage` of one id at the
/// commit, too).
#[test]
fn avatars_drawn_alike_add_one_picture() {
    let mut f = Fixture::new();
    let (list, _) = f.team();
    let plan = f.plan(&[list], &Options::default());
    let tx = f.transaction(&plan, Some(&mut Painter(0)));
    let added =
        tx.0.iter()
            .filter(|op| matches!(op, Operation::AddImage { .. }))
            .count();
    assert_eq!(added, 1, "one picture");
}

/// Changing one row's kind or the avatars switch reshuffles no other record's
/// person.
///
/// flip: `draw_record` seeding an entity by how many rows its record has → fails
/// at "the avatars switch moves no name".
#[test]
fn a_change_to_one_row_or_the_avatars_moves_no_other_person() {
    let mut f = Fixture::new();
    let (list, rows) = f.team();
    let base = f.plan(&[list], &Options::default());
    let names = |p: &Plan| f.column(p, &rows, "Name");
    let off = f.plan(
        &[list],
        &Options {
            avatars: None,
            ..Options::default()
        },
    );
    assert_eq!(
        names(&base),
        names(&off),
        "the avatars switch moves no name"
    );
    let mut one = Options::default();
    one.kinds.insert(f.child(rows[0], "Role"), Some(Kind::City));
    let changed = f.plan(&[list], &one);
    assert_eq!(
        names(&base)[1..],
        names(&changed)[1..],
        "another row's kind moves no one else"
    );
}

/// A list longer than a table still fills every row, and repeats an entry only
/// once the table's others are all taken.
///
/// flip: `Rng::pick_fresh` answering `""` once its list is used up → fails at
/// "every row filled".
#[test]
fn a_list_longer_than_its_table_fills_every_row() {
    let cities = section(include_str!("../assets/content/en-US.txt"), "cities").len();
    let mut f = Fixture::new();
    let (list, rows) = f.list(60, &["Name", "City"], None);
    let plan = f.plan(&[list], &opts("en-US"));
    let column = f.column(&plan, &rows, "City");
    assert!(column.iter().all(|c| !c.is_empty()), "every row filled");
    let first: std::collections::HashSet<&String> = column[..cities].iter().collect();
    assert_eq!(first.len(), cities, "no city twice before all are taken");
    assert!(
        f.column(&plan, &rows, "Name").iter().all(|n| !n.is_empty()),
        "every name filled"
    );
}

/// The gendered entries a locale ships agree with the person: in German and in
/// Spanish, a bio takes the form that agrees with the name it is about.
///
/// flip: `Drawn::fresh_gendered` taking the masculine form always → fails at the
/// agreement.
#[test]
fn a_bio_agrees_with_its_person() {
    for (tag, source) in [
        ("de", include_str!("../assets/content/de.txt")),
        ("es", include_str!("../assets/content/es.txt")),
    ] {
        let women: std::collections::HashSet<&str> =
            section(source, "given-f").into_iter().collect();
        let (mut masc, mut fem) = (
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        );
        let words = |s: &str| -> Vec<String> {
            s.split_whitespace()
                .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_string())
                .collect()
        };
        for pair in section(source, "bios") {
            if let Some((m, w)) = pair.split_once('|') {
                let (mw, ww) = (words(m), words(w));
                masc.extend(mw.iter().filter(|x| !ww.contains(x)).cloned());
                fem.extend(ww.iter().filter(|x| !mw.contains(x)).cloned());
            }
        }
        let mut f = Fixture::new();
        let (list, rows) = f.list(40, &["Name", "Bio"], None);
        let mut o = opts(tag);
        for row in &rows {
            o.kinds.insert(f.child(*row, "Bio"), Some(Kind::Bio));
        }
        let plan = f.plan(&[list], &o);
        let mut gendered = 0;
        for (name, bio) in f
            .column(&plan, &rows, "Name")
            .iter()
            .zip(f.column(&plan, &rows, "Bio"))
        {
            let ws = words(&bio);
            let (is_m, is_f) = (
                ws.iter().any(|w| masc.contains(w)),
                ws.iter().any(|w| fem.contains(w)),
            );
            if is_m || is_f {
                gendered += 1;
                let woman = women.contains(name.split(' ').next().unwrap());
                assert_eq!(is_f, woman, "{tag}: {name}, {bio}");
            }
        }
        assert!(gendered >= 4, "{tag} drew gendered bios: {gendered}");
    }
}

/// An unshipped locale falls back to the first shipped one in a plan, a text in
/// an unshipped Language disagrees with every shipped one, tags read without
/// case or with `_`, and the verb refuses an unshipped locale.
///
/// flip: `agreed_locale` skipping a text whose Language is not shipped → fails
/// at "an unshipped Language disagrees".
#[test]
fn the_locale_falls_back_and_disagrees_as_the_design_says() {
    let mut f = Fixture::new();
    let (list, rows) = f.list(3, &["Name", "City"], None);
    let first = &locales()[0].tag;
    let fallback = f.plan(&[list], &opts("ja"));
    let shipped = f.plan(&[list], &opts(first));
    assert_eq!(
        f.column(&fallback, &rows, "City"),
        f.column(&shipped, &rows, "City"),
        "an unshipped locale draws as the first"
    );

    let de = f.add(f.page, text("x", TextSizing::Auto, Some("de")), "Datum");
    let ja = f.add(f.page, text("x", TextSizing::Auto, Some("ja")), "Datum");
    assert!(
        fill::agreed_locale(&f.doc, &[de, ja]).is_none(),
        "an unshipped Language disagrees"
    );
    assert_eq!(locale_for("EN-gb").unwrap().tag, "en-GB");
    assert_eq!(locale_for("de_DE").unwrap().tag, "de");

    let res = Resolved::rebuild(&f.doc);
    let refused = verbs::call(
        &mut Ctx {
            doc: &f.doc,
            res: &res,
            ids: &mut f.ids,
            images: None,
            contrast: None,
        },
        "fill_content",
        &json!({ "nodes": [list.to_wire()], "locale": "ja" }),
    );
    assert!(
        matches!(
            refused,
            Err(verbs::VerbError::Invalid {
                param: "locale",
                ..
            })
        ),
        "{refused:?}"
    );
}

/// A text its frame's layout grows is fitted to the width it is drawn at, not
/// the width the document stores.
///
/// flip: `draw_record` measuring the document's kind rather than the used one →
/// fails at "fits the drawn width".
#[test]
fn a_grown_text_is_fitted_to_the_width_it_is_drawn_at() {
    use ondin_core::container::{Display, Flex, LayoutItem};
    let mut f = Fixture::new();
    let card = f.add(
        f.page,
        NodeKind::Artboard {
            size: Size::new(400.0, 80.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        "Card",
    );
    let name = f.add(
        card,
        text("Name", TextSizing::AutoHeight(40.0), None),
        "Full name",
    );
    f.commit(vec![
        Operation::SetDisplay {
            id: card,
            display: Some(Display::Flex(Flex {
                padding: [20.0; 4],
                ..Default::default()
            })),
        },
        Operation::SetLayoutItem {
            id: name,
            item: LayoutItem {
                grow: 1.0,
                ..Default::default()
            },
        },
    ]);
    let res = Resolved::rebuild(&f.doc);
    let drawn = match res.used_kind(&f.doc, name) {
        Some(NodeKind::Text {
            sizing: TextSizing::AutoHeight(w),
            ..
        }) => *w,
        Some(NodeKind::Text {
            sizing: TextSizing::Fixed(s),
            ..
        }) => s.width,
        other => panic!("{other:?}"),
    };
    assert!(drawn > 300.0, "fixture: grown to {drawn}");
    for seed in 0..5 {
        let plan = f.plan(
            &[card],
            &Options {
                seed,
                ..Options::default()
            },
        );
        let row = plan.rows.iter().find(|r| r.node == name).unwrap();
        assert!(
            row.fits,
            "seed {seed} fits the drawn width: {:?}",
            row.value
        );
    }
}
