//! *Fill with content…*: realistic names, addresses, job titles, companies, text,
//! numbers and avatars for the text layers of a selection, a different value per
//! layer, drawn from the tables in `assets/content` (one file per locale, so the
//! fill works offline and a locale is added without code).
//!
//! [`plan`] reads each layer's kind from its name, then its text; groups layers
//! into records whose values agree (a row's email is built from its name); draws
//! every value from a seed, so the same seed gives the same fill; and redraws a
//! value that would overflow a fixed-width line. An avatar is vector art
//! ([`Avatar::art`]) that the caller's [`ImageSink::draw`] turns into a picture.

use crate::build::{all_in_document_order, outermost};
use crate::component::main_of;
use crate::node::{NodeKind, TextRef, TextSizing};
use crate::query::is_effectively_locked;
use crate::svg_in::ImageSink;
use crate::{Document, Fill, NodeId, Operation, Transaction, keyed_by_position};
use kurbo::{BezPath, Circle, Rect, Shape};
use peniko::Color;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

// --- the tables ---------------------------------------------------------------

/// The shipped tables, in the order the locale list shows them. The first of a
/// language is the one its bare tag means (`en` is English (US)).
static SOURCES: [&str; 8] = [
    include_str!("../assets/content/en-US.txt"),
    include_str!("../assets/content/en-GB.txt"),
    include_str!("../assets/content/de.txt"),
    include_str!("../assets/content/fr.txt"),
    include_str!("../assets/content/es.txt"),
    include_str!("../assets/content/it.txt"),
    include_str!("../assets/content/pt-BR.txt"),
    include_str!("../assets/content/nl.txt"),
];

/// One locale's tables: its formats (`@key value`) and lists (`[section]`), with
/// whatever it leaves out read from the locale it names in `@inherit`.
#[derive(Debug)]
pub struct Locale {
    /// The BCP-47 tag (`en-US`, `de`), matched against a text's Language.
    pub tag: String,
    /// How the locale list names it, in English as the Type panel names languages.
    pub name: String,
    formats: HashMap<String, String>,
    lists: HashMap<String, Vec<String>>,
}

impl Locale {
    /// A format, or `""` when neither this locale nor the one it inherits from
    /// sets it.
    fn format(&self, key: &str) -> &str {
        self.formats.get(key).map_or("", String::as_str)
    }

    fn list(&self, key: &str) -> &[String] {
        self.lists.get(key).map_or(&[], Vec::as_slice)
    }
}

/// `\uXXXX` in a value is that character, so a no-break space can be written.
fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find("\\u") {
        out.push_str(&rest[..at]);
        let hex = rest.get(at + 2..at + 6).unwrap_or("");
        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
            Some(c) => {
                out.push(c);
                rest = &rest[at + 6..];
            }
            None => {
                out.push_str("\\u");
                rest = &rest[at + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// One table file read: `#` lines are comments, `@key value` a format,
/// `[name]` starts a list whose entries are the lines under it.
fn parse(source: &str) -> (Locale, Option<String>) {
    let mut formats = HashMap::new();
    let mut lists: HashMap<String, Vec<String>> = HashMap::new();
    let mut section: Option<String> = None;
    for line in source.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('@') {
            let (key, value) = rest.split_once(' ').unwrap_or((rest, ""));
            formats.insert(key.to_string(), unescape(value.trim()));
        } else if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = Some(name.to_string());
            lists.entry(name.to_string()).or_default();
        } else if let Some(name) = &section {
            lists
                .entry(name.clone())
                .or_default()
                .push(unescape(line.trim()));
        }
    }
    let inherit = formats.remove("inherit");
    let locale = Locale {
        tag: formats.remove("tag").unwrap_or_default(),
        name: formats.remove("name").unwrap_or_default(),
        formats,
        lists,
    };
    (locale, inherit)
}

/// A locale's formats and lists, as another inherits them.
type Tables = (HashMap<String, String>, HashMap<String, Vec<String>>);

static LOCALES: LazyLock<Vec<Locale>> = LazyLock::new(|| {
    let parsed: Vec<(Locale, Option<String>)> = SOURCES.iter().map(|s| parse(s)).collect();
    // What each locale inherits, copied out first: one level, which is all the
    // tables use.
    let inherited: Vec<Option<Tables>> = parsed
        .iter()
        .map(|(_, inherit)| {
            let parent = parsed
                .iter()
                .find(|(p, _)| Some(&p.tag) == inherit.as_ref())?;
            Some((parent.0.formats.clone(), parent.0.lists.clone()))
        })
        .collect();
    parsed
        .into_iter()
        .zip(inherited)
        .map(|((mut locale, _), parent)| {
            if let Some((formats, lists)) = parent {
                for (k, v) in formats {
                    locale.formats.entry(k).or_insert(v);
                }
                for (k, v) in lists {
                    locale.lists.entry(k).or_insert(v);
                }
            }
            locale
        })
        .collect()
});

/// Every shipped locale, in the order the locale list shows them.
pub fn locales() -> &'static [Locale] {
    &LOCALES
}

/// The locale for a BCP-47 tag: the one with that exact tag, else the first of
/// its language (`de-AT` → German, `en` → English (US)).
pub fn locale_for(tag: &str) -> Option<&'static Locale> {
    let tag = tag.trim();
    let primary = |t: &str| {
        t.split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    locales()
        .iter()
        .find(|l| l.tag.eq_ignore_ascii_case(tag))
        .or_else(|| locales().iter().find(|l| primary(&l.tag) == primary(tag)))
}

// --- kinds --------------------------------------------------------------------

/// What a layer is filled with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    FullName,
    FirstName,
    LastName,
    Email,
    Phone,
    JobTitle,
    Department,
    Username,
    Street,
    City,
    Postcode,
    Country,
    Address,
    Company,
    Product,
    Headline,
    Sentence,
    Paragraph,
    /// A short third-person bio of the record's person.
    Bio,
    /// A customer's testimonial, in the first person.
    Quote,
    Price,
    Number,
    Percent,
    Date,
    Time,
    /// A shape's picture: the record's initials or a pattern, never text.
    Avatar,
}

/// Each kind's key, in [`Kind`]'s declaration order, which [`Kind::ALL`] keeps.
pub const KIND_KEYS: [&str; 26] = [
    "full_name",
    "first_name",
    "last_name",
    "email",
    "phone",
    "job_title",
    "department",
    "username",
    "street",
    "city",
    "postcode",
    "country",
    "address",
    "company",
    "product",
    "headline",
    "sentence",
    "paragraph",
    "bio",
    "quote",
    "price",
    "number",
    "percent",
    "date",
    "time",
    "avatar",
];

/// The heading a kind is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Person,
    Place,
    Company,
    Text,
    Numbers,
    Image,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Person => "Person",
            Group::Place => "Place",
            Group::Company => "Company",
            Group::Text => "Text",
            Group::Numbers => "Numbers and dates",
            Group::Image => "Image",
        }
    }
}

impl Kind {
    /// Every kind, in the order the kind list shows them, grouped.
    pub const ALL: [Kind; 26] = [
        Kind::FullName,
        Kind::FirstName,
        Kind::LastName,
        Kind::Email,
        Kind::Phone,
        Kind::JobTitle,
        Kind::Department,
        Kind::Username,
        Kind::Street,
        Kind::City,
        Kind::Postcode,
        Kind::Country,
        Kind::Address,
        Kind::Company,
        Kind::Product,
        Kind::Headline,
        Kind::Sentence,
        Kind::Paragraph,
        Kind::Bio,
        Kind::Quote,
        Kind::Price,
        Kind::Number,
        Kind::Percent,
        Kind::Date,
        Kind::Time,
        Kind::Avatar,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kind::FullName => "Full name",
            Kind::FirstName => "First name",
            Kind::LastName => "Last name",
            Kind::Email => "Email",
            Kind::Phone => "Phone",
            Kind::JobTitle => "Job title",
            Kind::Department => "Department",
            Kind::Username => "Username",
            Kind::Street => "Street address",
            Kind::City => "City",
            Kind::Postcode => "Postcode",
            Kind::Country => "Country",
            Kind::Address => "Full address",
            Kind::Company => "Company",
            Kind::Product => "Product",
            Kind::Headline => "Headline",
            Kind::Sentence => "Sentence",
            Kind::Paragraph => "Paragraph",
            Kind::Bio => "Bio",
            Kind::Quote => "Quote",
            Kind::Price => "Price",
            Kind::Number => "Number",
            Kind::Percent => "Percent",
            Kind::Date => "Date",
            Kind::Time => "Time",
            Kind::Avatar => "Avatar",
        }
    }

    /// How the verb and prefs spell it: its entry in [`KIND_KEYS`].
    pub fn key(self) -> &'static str {
        KIND_KEYS[self as usize]
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == key)
    }

    pub fn group(self) -> Group {
        match self {
            Kind::FullName
            | Kind::FirstName
            | Kind::LastName
            | Kind::Email
            | Kind::Phone
            | Kind::JobTitle
            | Kind::Department
            | Kind::Username => Group::Person,
            Kind::Street | Kind::City | Kind::Postcode | Kind::Country | Kind::Address => {
                Group::Place
            }
            Kind::Company | Kind::Product => Group::Company,
            Kind::Headline | Kind::Sentence | Kind::Paragraph | Kind::Bio | Kind::Quote => {
                Group::Text
            }
            Kind::Price | Kind::Number | Kind::Percent | Kind::Date | Kind::Time => Group::Numbers,
            Kind::Avatar => Group::Image,
        }
    }

    /// Whether the kind writes text, as every kind but [`Kind::Avatar`] does.
    pub fn is_text(self) -> bool {
        self != Kind::Avatar
    }
}

/// The value a record's rows agree on that a kind is drawn from, so redrawing
/// one row of it redraws them together and its email still matches its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Entity {
    Person,
    Place,
    Firm,
}

fn entity_of(kind: Kind) -> Option<Entity> {
    match kind {
        Kind::FullName
        | Kind::FirstName
        | Kind::LastName
        | Kind::Email
        | Kind::Username
        | Kind::Avatar => Some(Entity::Person),
        Kind::Street | Kind::City | Kind::Postcode | Kind::Address | Kind::Phone => {
            Some(Entity::Place)
        }
        Kind::Company => Some(Entity::Firm),
        _ => None,
    }
}

// --- reading a layer's kind ---------------------------------------------------

/// The words of a name, lowercased, digits and punctuation dropped.
fn words(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether `phrase`'s words appear in `ws`, consecutively.
fn has_phrase(ws: &[String], phrase: &str) -> bool {
    let p: Vec<&str> = phrase.split(' ').collect();
    ws.windows(p.len())
        .any(|w| w.iter().zip(&p).all(|(a, b)| a == b))
}

/// The phrases a text layer's name is read for, most specific first: *Job
/// title* is a job before *title* is a headline, *Company name* a company before
/// *name* is a person, *Street address* a street before *address* is a whole one.
const NAME_RULES: &[(Kind, &[&str])] = &[
    (
        Kind::Username,
        &[
            "username",
            "user name",
            "handle",
            "nickname",
            "benutzername",
            "usuario",
            "utente",
        ],
    ),
    (
        Kind::FirstName,
        &[
            "first name",
            "firstname",
            "given name",
            "forename",
            "vorname",
            "prénom",
            "prenom",
            "nombre de pila",
            "nome proprio",
            "primeiro nome",
            "voornaam",
        ],
    ),
    (
        Kind::LastName,
        &[
            "last name",
            "lastname",
            "surname",
            "family name",
            "nachname",
            "nom de famille",
            "apellido",
            "apellidos",
            "cognome",
            "sobrenome",
            "achternaam",
        ],
    ),
    (
        Kind::FullName,
        &[
            "full name",
            "fullname",
            "display name",
            "nombre completo",
            "nome completo",
            "nom complet",
            "volledige naam",
            "author",
            "autor",
            "auteur",
            "autore",
            "team member",
            "member",
        ],
    ),
    (
        Kind::Email,
        &["email", "e mail", "mail", "correo", "courriel"],
    ),
    (
        Kind::Phone,
        &[
            "phone",
            "telephone",
            "tel",
            "mobile",
            "cell phone",
            "cellphone",
            "telefon",
            "handy",
            "téléphone",
            "teléfono",
            "telefono",
            "telefone",
            "celular",
            "móvil",
            "cellulare",
            "telefoon",
            "mobiel",
        ],
    ),
    (
        Kind::JobTitle,
        &[
            "job",
            "job title",
            "role",
            "position",
            "occupation",
            "beruf",
            "rolle",
            "poste",
            "fonction",
            "métier",
            "puesto",
            "cargo",
            "ruolo",
            "professione",
            "função",
            "profissão",
            "functie",
            "beroep",
        ],
    ),
    (
        Kind::Department,
        &[
            "department",
            "dept",
            "team",
            "division",
            "abteilung",
            "département",
            "departamento",
            "reparto",
            "dipartimento",
            "setor",
            "afdeling",
        ],
    ),
    (
        Kind::Company,
        &[
            "company",
            "organization",
            "organisation",
            "org",
            "employer",
            "business",
            "firma",
            "unternehmen",
            "entreprise",
            "société",
            "empresa",
            "compañía",
            "azienda",
            "società",
            "bedrijf",
            "organisatie",
        ],
    ),
    (
        Kind::Price,
        &[
            "price", "cost", "amount", "total", "subtotal", "fee", "preis", "betrag", "prix",
            "montant", "precio", "importe", "prezzo", "importo", "preço", "valor", "prijs",
            "bedrag",
        ],
    ),
    (
        Kind::Product,
        &[
            "product", "app name", "produkt", "produit", "producto", "prodotto", "produto",
        ],
    ),
    (
        Kind::Street,
        &[
            "street", "straße", "strasse", "rue", "calle", "rua", "straat",
        ],
    ),
    (
        Kind::Address,
        &[
            "address",
            "adresse",
            "anschrift",
            "dirección",
            "direccion",
            "indirizzo",
            "endereço",
            "endereco",
            "adres",
        ],
    ),
    (
        Kind::Postcode,
        &[
            "zip",
            "zip code",
            "postcode",
            "postal code",
            "postal",
            "plz",
            "postleitzahl",
            "code postal",
            "código postal",
            "codice postale",
            "cep",
        ],
    ),
    (
        Kind::City,
        &[
            "city", "town", "location", "stadt", "ort", "ville", "ciudad", "città", "citta",
            "cidade", "stad", "plaats",
        ],
    ),
    (
        Kind::Country,
        &[
            "country", "land", "pays", "país", "pais", "paese", "nazione",
        ],
    ),
    (
        Kind::Percent,
        &[
            "percent",
            "percentage",
            "discount",
            "prozent",
            "pourcentage",
            "porcentaje",
            "percentuale",
            "porcentagem",
            "procent",
        ],
    ),
    (
        Kind::Time,
        &[
            "time", "hour", "uhrzeit", "zeit", "heure", "hora", "ora", "tijd",
        ],
    ),
    (
        Kind::Date,
        &[
            "date",
            "day",
            "datum",
            "fecha",
            "due",
            "deadline",
            "birthday",
            "born",
            "dob",
            "created",
            "updated",
            "joined",
            "published",
            "posted",
        ],
    ),
    (
        Kind::Number,
        &[
            "number",
            "count",
            "qty",
            "quantity",
            "views",
            "followers",
            "likes",
            "score",
            "anzahl",
            "número",
            "numero",
            "aantal",
        ],
    ),
    (
        Kind::Headline,
        &[
            "headline", "title", "hero", "titel", "titre", "título", "titolo",
        ],
    ),
    (
        Kind::Sentence,
        &[
            "subtitle",
            "subheading",
            "subhead",
            "tagline",
            "caption",
            "sentence",
            "summary",
            "teaser",
            "excerpt",
        ],
    ),
    (
        Kind::Bio,
        &[
            "bio",
            "biography",
            "about me",
            "biografie",
            "biographie",
            "biografía",
            "biografia",
        ],
    ),
    (
        Kind::Quote,
        &[
            "quote",
            "testimonial",
            "review",
            "zitat",
            "citation",
            "témoignage",
            "cita",
            "testimonio",
            "reseña",
            "citazione",
            "testimonianza",
            "recensione",
            "citação",
            "depoimento",
            "citaat",
            "recensie",
        ],
    ),
    (
        Kind::Paragraph,
        &[
            "paragraph",
            "body",
            "description",
            "copy",
            "about",
            "content",
            "lorem",
            "beschreibung",
            "texte",
            "descripción",
            "descrizione",
            "descrição",
            "beschrijving",
            "omschrijving",
        ],
    ),
    (
        Kind::FullName,
        &[
            "name", "nombre", "nom", "nome", "naam", "person", "contact", "customer",
        ],
    ),
];

/// The words that make a shape an avatar wherever they stand in its name.
const AVATAR_WORDS: &[&str] = &["avatar", "headshot", "pfp", "portrait"];

/// The words that make a shape an avatar only when every word of its name is
/// one of these or [`AVATAR_FILLER`]: *Profile picture* is one, *Profile header
/// bg* is not.
const AVATAR_WEAK: &[&str] = &["photo", "picture", "pic", "profile", "foto", "bild"];

/// The words an avatar's name may carry beside [`AVATAR_WEAK`].
const AVATAR_FILLER: &[&str] = &[
    "user", "author", "member", "person", "account", "image", "img",
];

/// A text layer's kind read from its name, or `None` when no rule matches.
pub fn kind_from_name(name: &str) -> Option<Kind> {
    let ws = words(name);
    NAME_RULES
        .iter()
        .find(|(_, phrases)| phrases.iter().any(|p| has_phrase(&ws, p)))
        .map(|(k, _)| *k)
}

/// Whether a shape's name makes it an avatar.
pub fn is_avatar_name(name: &str) -> bool {
    let ws = words(name);
    let is = |list: &[&str], w: &String| list.contains(&w.as_str());
    ws.iter().any(|w| is(AVATAR_WORDS, w))
        || (ws.iter().any(|w| is(AVATAR_WEAK, w))
            && ws
                .iter()
                .all(|w| is(AVATAR_WEAK, w) || is(AVATAR_FILLER, w)))
}

/// Whether a layer's name only repeats its text, as an importer names every
/// text layer. Such a name may be a placeholder's (*Name* in a card) or a
/// label's (*Email* over a field), so it is read with care.
fn named_like_text(name: &str, content: &str) -> bool {
    let named = words(name);
    !named.is_empty() && named == words(content)
}

/// A text layer's kind: read from its name, then from its text. A name that
/// only repeats a text of three words or fewer is not read as a paragraph,
/// sentence, bio or quote: *About* in a nav bar and *Copy link* on a button are
/// labels, not body copy.
pub fn kind_of_text_layer(name: &str, content: &str) -> Option<Kind> {
    let long = |k: &Kind| {
        matches!(
            k,
            Kind::Paragraph | Kind::Sentence | Kind::Bio | Kind::Quote
        )
    };
    let short_label = named_like_text(name, content) && words(content).len() <= 3;
    kind_from_name(name)
        .filter(|k| !(short_label && long(k)))
        .or_else(|| kind_from_text(content))
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[^\s@]+@[^\s@]+$").unwrap());
static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:R\$|\p{Sc})\s?[+-]?\d[\d.,\s\u{A0}\u{202F}]*$|^[+-]?\d[\d.,\s\u{A0}\u{202F}]*\s?\p{Sc}$")
        .unwrap()
});
static PERCENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[+-]?\d[\d.,]*\s?\u{A0}?%$").unwrap());
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{1,2}[:.]\d{2}(\s?[AaPp]\.?[Mm]\.?)?$").unwrap());
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\d{1,4}[./-]\d{1,2}[./-]\d{1,4}$|^\p{L}{3,9}\.? \d{1,2},? \d{4}$|^\d{1,2}\.? \p{L}{3,9}\.? \d{4}$",
    )
    .unwrap()
});
static PHONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\+?[\d\s().\-]{7,}$").unwrap());
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?\d[\d.,\s]*$").unwrap());

/// A text layer's kind read from what it says: `@` is an email, a currency an
/// amount, a run of digits a phone number. `None` when it reads as none of them.
pub fn kind_from_text(text: &str) -> Option<Kind> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if t.to_lowercase().starts_with("lorem ipsum") {
        return Some(Kind::Paragraph);
    }
    let digits = t.chars().filter(char::is_ascii_digit).count();
    if EMAIL.is_match(t) {
        Some(Kind::Email)
    } else if PRICE.is_match(t) {
        Some(Kind::Price)
    } else if PERCENT.is_match(t) {
        Some(Kind::Percent)
    } else if TIME.is_match(t) {
        Some(Kind::Time)
    } else if DATE.is_match(t) {
        Some(Kind::Date)
    } else if PHONE.is_match(t) && digits >= 7 {
        Some(Kind::Phone)
    } else if NUMBER.is_match(t) {
        Some(Kind::Number)
    } else {
        None
    }
}

// --- drawing ------------------------------------------------------------------

/// A splitmix64 stream: small, seedable and the same on every machine.
struct Rng(u64);

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a over a string, for seeding from names and ids.
fn hash_str(s: &str) -> u64 {
    s.bytes().fold(0xCBF2_9CE4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01B3)
    })
}

impl Rng {
    fn new(parts: &[u64]) -> Self {
        Rng(parts.iter().fold(0x9E37_79B9_7F4A_7C15, |h, p| {
            mix(h ^ p.wrapping_add(0x9E37_79B9))
        }))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix(self.0)
    }

    fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next() % n }
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.below(hi.saturating_sub(lo) + 1)
    }

    fn pick<'a>(&mut self, xs: &'a [String]) -> &'a str {
        match xs.len() {
            0 => "",
            n => &xs[self.below(n as u64) as usize],
        }
    }

    fn chance(&mut self, one_in: u64) -> bool {
        self.below(one_in) == 0
    }

    /// An entry of `xs` that `taken` does not hold, while one is left: the
    /// drawn one, or the next free one after it. Always one draw, so the
    /// stream stays in step whatever is taken.
    fn pick_fresh<'a>(&mut self, xs: &'a [String], taken: &HashSet<String>) -> &'a str {
        let n = xs.len();
        if n == 0 {
            return "";
        }
        let start = self.below(n as u64) as usize;
        (0..n)
            .map(|k| xs[(start + k) % n].as_str())
            .find(|x| !taken.contains(*x))
            .unwrap_or(&xs[start])
    }
}

/// What the records drawn before take from each list, by the list's name, so a
/// later record passes over them while its list has others.
type Taken = HashMap<&'static str, HashSet<String>>;

/// The entries a draw took, by list, for [`Taken`].
type Picks = Vec<(&'static str, String)>;

/// `taken`'s entries of `list`, with those `picks` already holds.
fn taken_with(taken: &Taken, picks: &Picks, list: &str) -> HashSet<String> {
    let mut out = taken.get(list).cloned().unwrap_or_default();
    out.extend(
        picks
            .iter()
            .filter(|(l, _)| *l == list)
            .map(|(_, v)| v.clone()),
    );
    out
}

/// The letters `@` draws in a pattern: those a UK postcode's inward code uses.
const LETTERS: &[u8] = b"ABDEFGHJLNPQRSTUWXYZ";

/// A pattern filled: `#` a digit, `&` a digit but 0, `@` a capital, and a whole
/// `lo..hi` a number in that range at `lo`'s width.
fn fill_pattern(pattern: &str, rng: &mut Rng) -> String {
    if let Some((lo, hi)) = pattern.split_once("..")
        && let (Ok(a), Ok(b)) = (lo.parse::<u64>(), hi.parse::<u64>())
    {
        return format!("{:0width$}", rng.range(a, b), width = lo.len());
    }
    pattern
        .chars()
        .map(|c| match c {
            '#' => char::from(b'0' + rng.below(10) as u8),
            '&' => char::from(b'1' + rng.below(9) as u8),
            '@' => char::from(LETTERS[rng.below(LETTERS.len() as u64) as usize]),
            c => c,
        })
        .collect()
}

/// Whom a gendered entry agrees with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gender {
    Female,
    Male,
}

/// An entry written `masculine|feminine` in the form `gender` takes, or the
/// entry itself when it has one form. A person the locale gives no gender
/// takes the first.
fn gendered(entry: &str, gender: Option<Gender>) -> &str {
    match (entry.split_once('|'), gender) {
        (Some((_, f)), Some(Gender::Female)) => f,
        (Some((m, _)), _) => m,
        (None, _) => entry,
    }
}

/// `{key}` in `template` replaced by its value; an unknown key is left.
fn render(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (k, v) in values {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// A name folded to what an email address or handle holds: lowercase ASCII
/// letters and digits.
fn ascii_fold(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        let folded = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
            'æ' => "ae",
            'ç' => "c",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'ı' => "i",
            'ñ' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
            'œ' => "oe",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'ý' | 'ÿ' => "y",
            'ß' => "ss",
            'ł' => "l",
            c if c.is_ascii_alphanumeric() => {
                out.push(c);
                continue;
            }
            _ => "",
        };
        out.push_str(folded);
    }
    out
}

/// `n` with the locale's thousands separator.
fn grouped(n: u64, sep: &str) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push_str(sep);
        }
        out.push(c);
    }
    out
}

/// A record's person.
#[derive(Clone, Debug)]
struct Person {
    given: String,
    family: String,
    /// `None` where the locale lists its given names ungendered.
    gender: Option<Gender>,
    /// Which of the address forms its email takes.
    email_form: u64,
    /// Which of the handle forms its username takes.
    handle_form: u64,
    /// The two digits a handle may end in.
    handle_no: u64,
}

impl Person {
    /// A person whose given and family names no earlier record took, while the
    /// lists have others. A locale lists given names as `given-f` and `given-m`,
    /// or ungendered as `given`.
    fn draw(locale: &Locale, rng: &mut Rng, taken: &Taken) -> Self {
        let none = HashSet::new();
        let given_taken = taken.get("given").unwrap_or(&none);
        let (female, male) = (locale.list("given-f"), locale.list("given-m"));
        let (given, gender) = match female.is_empty() || male.is_empty() {
            true => (rng.pick_fresh(locale.list("given"), given_taken), None),
            false => match rng.chance(2) {
                true => (rng.pick_fresh(female, given_taken), Some(Gender::Female)),
                false => (rng.pick_fresh(male, given_taken), Some(Gender::Male)),
            },
        };
        Person {
            given: given.to_string(),
            family: rng
                .pick_fresh(locale.list("family"), taken.get("family").unwrap_or(&none))
                .to_string(),
            gender,
            email_form: rng.below(3),
            handle_form: rng.below(5),
            handle_no: rng.range(10, 99),
        }
    }

    fn picks(&self) -> Picks {
        vec![
            ("given", self.given.clone()),
            ("family", self.family.clone()),
        ]
    }

    fn full(&self) -> String {
        format!("{} {}", self.given, self.family)
    }

    /// The first letter of the given name and of the family name's last word
    /// (*de Vries* → *V*).
    fn initials(&self) -> String {
        let first = |s: &str| {
            s.chars()
                .next()
                .map(|c| c.to_uppercase().collect::<String>())
        };
        let family = self.family.split(' ').next_back().unwrap_or("");
        [first(&self.given), first(family)]
            .into_iter()
            .flatten()
            .collect()
    }

    fn email(&self, domain: &str) -> String {
        let given = ascii_fold(&self.given);
        let family = ascii_fold(&self.family);
        let local = match self.email_form {
            0 => format!("{given}.{family}"),
            1 => format!("{}.{family}", given.chars().next().unwrap_or('x')),
            _ => given,
        };
        format!("{local}@{domain}")
    }

    fn username(&self) -> String {
        let given = ascii_fold(&self.given);
        let family = ascii_fold(&self.family);
        let initial = |s: &str| s.chars().next().unwrap_or('x');
        match self.handle_form {
            0 => format!("@{}{family}", initial(&given)),
            1 => format!("@{given}{family}"),
            2 => format!("@{given}.{family}"),
            3 => format!("@{given}_{}", initial(&family)),
            _ => format!("@{given}{}", self.handle_no),
        }
    }
}

/// A record's address.
#[derive(Clone, Debug)]
struct Place {
    /// Its row of the `cities` list, which [`Taken`] holds.
    row: String,
    street: String,
    unit: Option<String>,
    city: String,
    region: String,
    postcode: String,
    area: String,
    country: String,
}

impl Place {
    /// A place in a city no earlier record took, while the list has others.
    fn draw(locale: &Locale, rng: &mut Rng, taken: &Taken) -> Self {
        let none = HashSet::new();
        let row = rng
            .pick_fresh(locale.list("cities"), taken.get("cities").unwrap_or(&none))
            .to_string();
        let mut cols = row.split('|');
        let mut col = || cols.next().unwrap_or("").trim().to_string();
        let (city, region, code, area) = (col(), col(), col(), col());
        let house = locale.format("house");
        let house = fill_pattern(if house.is_empty() { "1..120" } else { house }, rng);
        let street = render(
            locale.format("street"),
            &[
                ("no", &house),
                ("street", rng.pick(locale.list("streets"))),
                ("suffix", rng.pick(locale.list("suffixes"))),
            ],
        );
        let unit = rng.chance(3).then(|| {
            let no = rng.range(1, 12).to_string();
            render(locale.format("unit"), &[("no2", &no)])
        });
        Place {
            row,
            street,
            unit,
            city,
            region,
            postcode: fill_pattern(&code, rng),
            area,
            country: locale.format("country").to_string(),
        }
    }

    fn picks(&self) -> Picks {
        vec![("cities", self.row.clone())]
    }

    fn address(&self, locale: &Locale) -> String {
        let street = match &self.unit {
            Some(u) => format!("{}, {u}", self.street),
            None => self.street.clone(),
        };
        render(
            locale.format("address"),
            &[
                ("street", &street),
                ("city", &self.city),
                ("region", &self.region),
                ("postcode", &self.postcode),
                ("country", &self.country),
            ],
        )
        .split('|')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// A number in the city's area. A format may list alternatives split by `|`,
    /// as a locale whose numbers reserved for fiction come in several blocks does.
    fn phone(&self, locale: &Locale, mobile: bool, rng: &mut Rng) -> String {
        let format = match mobile {
            true => locale.format("mobile"),
            false => locale.format("phone"),
        };
        let blocks: Vec<String> = format.split('|').map(str::to_string).collect();
        let pattern = rng.pick(&blocks).to_string();
        fill_pattern(&render(&pattern, &[("area", &self.area)]), rng)
    }
}

/// A company: its name, the legal form some records print, and its domain.
#[derive(Clone, Debug)]
struct Firm {
    name: String,
    legal: Option<String>,
    domain: String,
}

impl Firm {
    /// A company no earlier record named, after a few tries.
    fn draw(locale: &Locale, rng: &mut Rng, taken: &Taken) -> Self {
        let named = |rng: &mut Rng| {
            format!(
                "{}{}",
                rng.pick(locale.list("company-a")),
                rng.pick(locale.list("company-b"))
            )
        };
        let mut name = named(rng);
        for _ in 0..8 {
            if !taken.get("firm").is_some_and(|t| t.contains(&name)) {
                break;
            }
            name = named(rng);
        }
        // One in three prints its legal form (*Northwind Inc.*).
        let forms: Vec<String> = locale
            .format("legal")
            .split('|')
            .map(str::to_string)
            .collect();
        let legal = rng
            .chance(3)
            .then(|| rng.pick(&forms).to_string())
            .filter(|l| !l.is_empty());
        let domain = format!("{}.{}", ascii_fold(&name), locale.format("tld"));
        Firm {
            name,
            legal,
            domain,
        }
    }

    fn picks(&self) -> Picks {
        vec![("firm", self.name.clone())]
    }

    fn label(&self) -> String {
        match &self.legal {
            Some(l) => format!("{} {l}", self.name),
            None => self.name.clone(),
        }
    }
}

fn product(locale: &Locale, rng: &mut Rng) -> String {
    format!(
        "{} {}",
        rng.pick(locale.list("product-a")),
        rng.pick(locale.list("product-b"))
    )
}

/// The domain every email takes when [`Options::example_emails`] is on: reserved
/// for examples, so no address a mockup shows can reach anyone.
const EXAMPLE_DOMAIN: &str = "example.com";

/// What a record's values are drawn from.
struct Drawn<'a> {
    locale: &'a Locale,
    person: Person,
    place: Place,
    firm: Firm,
    /// The company the whole fill's emails share when the record names none.
    domain: &'a str,
    /// Whether the record has a company row, whose domain its emails then take.
    own_firm: bool,
    /// Every email at [`EXAMPLE_DOMAIN`].
    example_emails: bool,
    /// What the records drawn before took, which a row's own draws pass over.
    taken: &'a Taken,
}

impl Drawn<'_> {
    fn domain(&self) -> &str {
        match (self.example_emails, self.own_firm) {
            (true, _) => EXAMPLE_DOMAIN,
            (false, true) => &self.firm.domain,
            (false, false) => self.domain,
        }
    }

    /// An entry of `list` that neither an earlier record nor this row took.
    fn fresh(&self, list: &'static str, rng: &mut Rng, picks: &mut Picks) -> String {
        let taken = taken_with(self.taken, picks, list);
        let entry = rng.pick_fresh(self.locale.list(list), &taken).to_string();
        picks.push((list, entry.clone()));
        entry
    }

    /// `fresh`'s entry in the form the record's person takes.
    fn fresh_gendered(&self, list: &'static str, rng: &mut Rng, picks: &mut Picks) -> String {
        let entry = self.fresh(list, rng, picks);
        gendered(&entry, self.person.gender).to_string()
    }

    /// A product no earlier record named, after a few tries.
    fn product(&self, rng: &mut Rng, picks: &mut Picks) -> String {
        let taken = taken_with(self.taken, picks, "product");
        let mut name = product(self.locale, rng);
        for _ in 0..8 {
            if !taken.contains(&name) {
                break;
            }
            name = product(self.locale, rng);
        }
        picks.push(("product", name.clone()));
        name
    }

    /// `template` with its slots filled from the record.
    fn slots(&self, template: &str, rng: &mut Rng) -> String {
        let product = product(self.locale, rng);
        render(
            template,
            &[
                ("company", &self.firm.name),
                ("product", &product),
                ("city", &self.place.city),
                ("first", &self.person.given),
            ],
        )
    }

    /// One sentence from the bank, its slots filled from the record.
    fn sentence(&self, rng: &mut Rng, picks: &mut Picks) -> String {
        let template = self.fresh("sentences", rng, picks);
        self.slots(&template, rng)
    }

    /// A text kind's value for the layer named `name`, whose words ask a phone
    /// for a mobile number and a date for a birthday or a past day. The list
    /// entries it takes are added to `picks`.
    fn text(&self, kind: Kind, name: &str, rng: &mut Rng, picks: &mut Picks) -> String {
        let l = self.locale;
        match kind {
            Kind::FullName => self.person.full(),
            Kind::FirstName => self.person.given.clone(),
            Kind::LastName => self.person.family.clone(),
            Kind::Email => self.person.email(self.domain()),
            Kind::Username => self.person.username(),
            Kind::Phone => self.place.phone(l, is_mobile(name), rng),
            Kind::JobTitle => self.fresh_gendered("jobs", rng, picks),
            // Departments repeat down a real list, so they are not kept apart.
            Kind::Department => rng.pick(l.list("departments")).to_string(),
            Kind::Street => self.place.street.clone(),
            Kind::City => self.place.city.clone(),
            Kind::Postcode => self.place.postcode.clone(),
            Kind::Country => self.place.country.clone(),
            Kind::Address => self.place.address(l),
            Kind::Company => self.firm.label(),
            Kind::Product => self.product(rng, picks),
            Kind::Headline => self.fresh("headlines", rng, picks),
            Kind::Sentence | Kind::Paragraph => self.sentence(rng, picks),
            Kind::Bio => {
                let template = self.fresh_gendered("bios", rng, picks);
                self.slots(&template, rng)
            }
            Kind::Quote => {
                let template = self.fresh_gendered("quotes", rng, picks);
                self.slots(&template, rng)
            }
            Kind::Price => {
                let whole = match rng.below(3) {
                    0 => rng.range(2, 99),
                    1 => rng.range(10, 999),
                    _ => rng.range(100, 4999),
                };
                let cents = if rng.chance(2) { "00" } else { "99" };
                let n = format!(
                    "{}{}{cents}",
                    grouped(whole, l.format("group")),
                    l.format("decimal")
                );
                render(l.format("price"), &[("n", &n)])
            }
            Kind::Number => {
                // As many one-digit as four-digit counts, as a page of counts has.
                let digits = rng.range(1, 4) as u32;
                let n = rng.range(10u64.pow(digits - 1), 10u64.pow(digits) - 1);
                grouped(n, l.format("group"))
            }
            Kind::Percent => render(l.format("percent"), &[("n", &rng.range(1, 99).to_string())]),
            Kind::Date => {
                let (from, to) = date_years(name);
                let (y, m, d) = (rng.range(from, to), rng.range(1, 12), rng.range(1, 28));
                let months = l.list("months");
                let mon = months.get(m as usize - 1).map_or("", String::as_str);
                render(
                    l.format("date"),
                    &[
                        ("d", &d.to_string()),
                        ("dd", &format!("{d:02}")),
                        ("mm", &format!("{m:02}")),
                        ("mon", mon),
                        ("yyyy", &y.to_string()),
                    ],
                )
            }
            Kind::Time => {
                let h = rng.range(7, 20);
                let m = rng.below(4) * 15;
                let h12 = if h > 12 { h - 12 } else { h };
                let ampm = if h >= 12 {
                    l.format("pm")
                } else {
                    l.format("am")
                };
                render(
                    l.format("time"),
                    &[
                        ("HH", &format!("{h:02}")),
                        ("h12", &h12.to_string()),
                        ("MM", &format!("{m:02}")),
                        ("ampm", ampm),
                    ],
                )
            }
            Kind::Avatar => String::new(),
        }
    }
}

// --- fitting ------------------------------------------------------------------

/// The room a text layer's box leaves.
#[derive(Clone, Copy, Debug)]
enum Room {
    /// Auto width: the value takes its natural length.
    Free,
    /// One line of this width.
    Line(f64),
    /// Wraps and grows downward; a paragraph keeps about this many characters,
    /// the length it has now.
    Wrap(usize),
    /// A fixed box this wide and tall.
    Area(f64, f64),
}

/// The slack a measurement is allowed before a value counts as overflowing.
const FIT_SLACK: f64 = 0.5;

/// How many times a value is drawn before an overflow is accepted.
const ATTEMPTS: u64 = 4;

/// `kind`'s text parts with `text` in place of its content, styled as the first
/// character was.
fn with_text<R>(
    kind: &NodeKind,
    text: &str,
    sizing: Option<TextSizing>,
    f: impl FnOnce(TextRef<'_>) -> R,
) -> Option<R> {
    let NodeKind::Text {
        content,
        style,
        spans,
        para_spans,
        paragraph,
        ..
    } = kind
    else {
        return None;
    };
    let base = TextRef::of(kind)?;
    let spans = spans.replaced(0, content.len(), text.len(), style);
    let para = para_spans.edited(0, content.len(), text.len(), paragraph);
    let sizing = sizing.unwrap_or(*base.sizing);
    Some(f(TextRef {
        content: text,
        spans: &spans,
        para_spans: &para,
        sizing: &sizing,
        ..base
    }))
}

fn room(kind: &NodeKind) -> Room {
    let NodeKind::Text {
        content, sizing, ..
    } = kind
    else {
        return Room::Free;
    };
    // One line's height in this style, against which a box is one line or more.
    let line = with_text(kind, "Xg", Some(TextSizing::Auto), |p| {
        crate::text::measure(p).height()
    })
    .unwrap_or(0.0);
    let height = |t: &str, w: f64| {
        with_text(kind, t, Some(TextSizing::AutoHeight(w)), |p| {
            crate::text::measure(p).height()
        })
        .unwrap_or(0.0)
    };
    match *sizing {
        TextSizing::Auto => Room::Free,
        TextSizing::AutoHeight(w) => {
            match content.contains('\n') || height(content, w) > line * 1.5 {
                true => Room::Wrap(content.chars().count()),
                false => Room::Line(w),
            }
        }
        TextSizing::Fixed(s) => match s.height >= line * 1.8 {
            true => Room::Area(s.width, s.height),
            false => Room::Line(s.width),
        },
    }
}

fn fits(kind: &NodeKind, room: Room, text: &str) -> bool {
    match room {
        Room::Free | Room::Wrap(..) => true,
        Room::Line(w) => {
            !text.contains('\n')
                && with_text(kind, text, Some(TextSizing::Auto), |p| {
                    crate::text::content_widths(p).1
                })
                .is_some_and(|width| width <= w + FIT_SLACK)
        }
        Room::Area(w, h) => with_text(kind, text, Some(TextSizing::AutoHeight(w)), |p| {
            crate::text::measure(p).height()
        })
        .is_some_and(|height| height <= h + FIT_SLACK),
    }
}

// --- avatars ------------------------------------------------------------------

/// How an avatar is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum AvatarStyle {
    /// The record's initials, white on one of [`GROUNDS`].
    #[default]
    Initials,
    /// Three shapes from [`GROUNDS`] on another of them.
    Pattern,
}

impl AvatarStyle {
    pub fn label(self) -> &'static str {
        match self {
            AvatarStyle::Initials => "Initials",
            AvatarStyle::Pattern => "Pattern",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            AvatarStyle::Initials => "initials",
            AvatarStyle::Pattern => "pattern",
        }
    }
}

/// The eight grounds an avatar is drawn on, each at least 4.5:1 against white so
/// white initials read on any of them.
pub const GROUNDS: [Color; 8] = [
    Color::from_rgb8(0x24, 0x59, 0xC4),
    Color::from_rgb8(0x0F, 0x76, 0x6E),
    Color::from_rgb8(0x2F, 0x7A, 0x3B),
    Color::from_rgb8(0x6D, 0x3F, 0xC0),
    Color::from_rgb8(0xB0, 0x30, 0x6A),
    Color::from_rgb8(0xC0, 0x39, 0x2B),
    Color::from_rgb8(0xA8, 0x4B, 0x0A),
    Color::from_rgb8(0x47, 0x55, 0x69),
];

/// An avatar for a person, seeded by the name, so the same person always gets
/// the same picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Avatar {
    pub name: String,
    pub initials: String,
    pub style: AvatarStyle,
}

impl Avatar {
    fn seed(&self) -> u64 {
        hash_str(&self.name)
    }

    /// The ground it is drawn on.
    pub fn ground(&self) -> Color {
        GROUNDS[(self.seed() % GROUNDS.len() as u64) as usize]
    }

    /// The picture as filled paths over a `w`×`h` box, bottom first: the ground,
    /// then the initials or the pattern's shapes.
    pub fn art(&self, w: f64, h: f64) -> Vec<(BezPath, Color)> {
        let ground = self.ground();
        let mut out = vec![(Rect::new(0.0, 0.0, w, h).to_path(0.1), ground)];
        let side = w.min(h);
        match self.style {
            AvatarStyle::Initials => {
                let style = crate::TextStyle {
                    font_size: side * 0.4,
                    weight: 600,
                    ..Default::default()
                };
                let (spans, para_spans) = (Default::default(), Default::default());
                let (paragraph, block) = (Default::default(), Default::default());
                let sizing = TextSizing::Auto;
                let layout = crate::text::layout(TextRef {
                    content: &self.initials,
                    style: &style,
                    spans: &spans,
                    para_spans: &para_spans,
                    paragraph: &paragraph,
                    block: &block,
                    sizing: &sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                });
                let mut ink = crate::text::outline(&layout);
                let b = ink.bounding_box();
                if b.width() > 0.0 {
                    ink.apply_affine(kurbo::Affine::translate((
                        w / 2.0 - b.center().x,
                        h / 2.0 - b.center().y,
                    )));
                    out.push((ink, Color::WHITE));
                }
            }
            AvatarStyle::Pattern => {
                let mut rng = Rng::new(&[self.seed(), 7]);
                let others: Vec<Color> = GROUNDS.into_iter().filter(|c| *c != ground).collect();
                for i in 0..3u64 {
                    let colour = others[rng.below(others.len() as u64) as usize];
                    let (cx, cy) = (
                        w * (rng.range(10, 90) as f64 / 100.0),
                        h * (rng.range(10, 90) as f64 / 100.0),
                    );
                    let r = side * (rng.range(18, 40) as f64 / 100.0);
                    let path = match (i + rng.below(3)) % 3 {
                        0 => Circle::new((cx, cy), r).to_path(0.1),
                        1 => {
                            let mut p = Rect::new(cx - r, cy - r, cx + r, cy + r).to_path(0.1);
                            let turn = rng.range(0, 90) as f64;
                            p.apply_affine(kurbo::Affine::rotate_about(
                                turn.to_radians(),
                                (cx, cy),
                            ));
                            p
                        }
                        _ => {
                            let mut p = BezPath::new();
                            p.move_to((cx, cy - r));
                            p.line_to((cx + r, cy + r * 0.8));
                            p.line_to((cx - r, cy + r * 0.8));
                            p.close_path();
                            p
                        }
                    };
                    out.push((path, colour));
                }
            }
        }
        out
    }
}

// --- the plan -----------------------------------------------------------------

/// What a fill is asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The locale's tag ([`Locale::tag`]).
    pub locale: String,
    /// One kind for every text layer, or `None` to read each from its name.
    pub content: Option<Kind>,
    /// Keep a record's values together.
    pub records: bool,
    /// How avatars are drawn, or `None` to leave avatar shapes out.
    pub avatars: Option<AvatarStyle>,
    /// The draw: the same seed gives the same values.
    pub seed: u64,
    /// A kind chosen for one layer, `None` meaning *Leave as is*, over the rest.
    pub kinds: HashMap<NodeId, Option<Kind>>,
    /// How many times each layer's value has been redrawn.
    pub redraws: HashMap<NodeId, u64>,
    /// Every email at `example.com`, which can reach no one, instead of a
    /// domain built from a company's name, which may be someone's.
    pub example_emails: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            locale: "en-US".into(),
            content: None,
            records: true,
            avatars: Some(AvatarStyle::Initials),
            seed: 0,
            kinds: HashMap::new(),
            redraws: HashMap::new(),
            example_emails: false,
        }
    }
}

/// What a layer is given.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Left as it is.
    Kept,
    Text(String),
    Avatar(Avatar),
}

/// One layer of the fill.
#[derive(Clone, Debug)]
pub struct Row {
    pub node: NodeId,
    /// Its record, an index into [`Plan::records`].
    pub record: usize,
    /// What it is filled with; `None` is *Leave as is*.
    pub kind: Option<Kind>,
    /// What its name or text says it is, which *By layer name* gives it.
    pub read: Option<Kind>,
    /// Whether it is a shape (an avatar's place) rather than text.
    pub shape: bool,
    pub value: Value,
    /// Whether the value fits its box; an overflow is kept after
    /// [`ATTEMPTS`] draws.
    pub fits: bool,
}

/// A group of rows whose values agree.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// The frame it is, or `None` for loose layers drawn together.
    pub root: Option<NodeId>,
}

/// The fill [`plan`] makes.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub rows: Vec<Row>,
    pub records: Vec<Record>,
    /// Text or avatar layers skipped because they are locked.
    pub locked: usize,
}

impl Plan {
    /// The rows that change.
    pub fn changes(&self) -> usize {
        self.rows.iter().filter(|r| r.value != Value::Kept).count()
    }

    /// The rows left as they are.
    pub fn left(&self) -> usize {
        self.rows.len() - self.changes()
    }

    /// The frames that are records and have a row that changes: what *in 3
    /// rows* counts.
    pub fn filled_records(&self) -> Vec<NodeId> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                let root = r.root?;
                self.rows
                    .iter()
                    .any(|row| row.record == i && row.value != Value::Kept)
                    .then_some(root)
            })
            .collect()
    }

    /// What a record is called in the counts: the frames' shared name when they
    /// share one word (*row*, *card*), else *record*. Singular.
    pub fn record_noun(&self, doc: &Document) -> String {
        let stems: HashSet<String> = self
            .filled_records()
            .iter()
            .filter_map(|id| doc.get(*id))
            .map(|n| stem(n.name()))
            .collect();
        match stems.into_iter().collect::<Vec<_>>().as_slice() {
            [one] if !one.is_empty() && one.chars().all(char::is_alphabetic) => one.clone(),
            _ => "record".to_string(),
        }
    }

    /// The fill as one transaction. A text takes its new content styled as its
    /// first character was; an avatar replaces the shape's fills with one picture
    /// drawn at twice the shape's size through `pictures`, and is left out when
    /// nothing is lent or it cannot draw.
    pub fn transaction(
        &self,
        doc: &Document,
        mut pictures: Option<&mut (dyn ImageSink + '_)>,
    ) -> Transaction {
        let mut ops = Vec::new();
        let mut added = HashSet::new();
        for row in &self.rows {
            let Some(node) = doc.get(row.node) else {
                continue;
            };
            match (&row.value, node.kind()) {
                (
                    Value::Text(text),
                    NodeKind::Text {
                        content,
                        style,
                        spans,
                        para_spans,
                        paragraph,
                        ..
                    },
                ) => ops.push(Operation::SetText {
                    id: row.node,
                    content: text.clone(),
                    spans: spans.replaced(0, content.len(), text.len(), style),
                    para_spans: para_spans.edited(0, content.len(), text.len(), paragraph),
                }),
                (Value::Avatar(avatar), kind) => {
                    let Some(sink) = pictures.as_deref_mut() else {
                        continue;
                    };
                    let size = kind.box_size().unwrap_or(kurbo::Size::new(32.0, 32.0));
                    let px = |v: f64| (v * 2.0).ceil().clamp(8.0, 2048.0);
                    let (w, h) = (px(size.width), px(size.height));
                    let Some((id, entry)) = sink.draw(w as u32, h as u32, &avatar.art(w, h)) else {
                        continue;
                    };
                    if !doc.has_image(&id) && added.insert(id.clone()) {
                        ops.push(Operation::AddImage {
                            id: id.clone(),
                            entry,
                        });
                    }
                    ops.push(Operation::SetFills {
                        id: row.node,
                        fills: keyed_by_position([Fill {
                            brush: crate::image_brush(id).into(),
                            visible: true,
                        }]),
                    });
                }
                _ => {}
            }
        }
        Transaction(ops)
    }
}

/// A frame's name without its trailing number, lowercased: *Row 2* → *row*.
fn stem(name: &str) -> String {
    name.trim_end_matches(|c: char| c.is_ascii_digit() || c.is_whitespace() || "#-_".contains(c))
        .trim()
        .to_lowercase()
}

/// A layer the fill may write, found under the selection.
struct Target {
    node: NodeId,
    shape: bool,
    read: Option<Kind>,
    /// A text named only by its own text ([`named_like_text`]), whose kind came
    /// from that name: a label if another layer shows the same kind.
    label_like: bool,
}

fn is_container(kind: &NodeKind) -> bool {
    matches!(kind, NodeKind::Artboard { .. } | NodeKind::Group)
}

/// Whether a layer is a shape the fill lists: a box whose fill a picture can
/// replace (a frame only while empty), named as an avatar or showing a picture.
fn is_shape_target(n: &crate::Node) -> bool {
    let shape = match n.kind() {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. } => true,
        NodeKind::Artboard { .. } => n.children().is_empty(),
        _ => false,
    };
    shape && (is_avatar_name(n.name()) || n.paint().image_fill().is_some())
}

/// The text layers under `roots`, and the shapes named as avatars or showing a
/// picture, in reading order (each parent's children first to last), with how
/// many such layers were skipped as locked. Hidden layers and texts bound to a
/// variable are passed over.
fn gather(doc: &Document, roots: &[NodeId]) -> (Vec<Target>, usize) {
    let roots = all_in_document_order(doc, &outermost(doc, roots));
    let mut out = Vec::new();
    let mut locked = 0;
    for root in roots {
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            if !doc.shown(id) {
                continue;
            }
            let target = match n.kind() {
                NodeKind::Text { content, .. }
                    if n.text_vars()[crate::variable::TEXT_CONTENT].is_none() =>
                {
                    let read = kind_of_text_layer(n.name(), content);
                    Some(Target {
                        node: id,
                        shape: false,
                        read,
                        label_like: named_like_text(n.name(), content)
                            && read.is_some()
                            && read != kind_from_text(content),
                    })
                }
                _ if is_shape_target(n) => Some(Target {
                    node: id,
                    shape: true,
                    read: is_avatar_name(n.name()).then_some(Kind::Avatar),
                    label_like: false,
                }),
                _ => None,
            };
            if let Some(t) = target {
                match is_effectively_locked(doc, id) {
                    true => locked += 1,
                    false => out.push(t),
                }
            }
            stack.extend(n.children().iter().rev());
        }
    }
    (out, locked)
}

/// The frames under `roots` that are records: each selected frame, and each
/// child of a frame whose children repeat (two or more with one main, or one
/// name and kind).
fn record_roots(doc: &Document, roots: &[NodeId]) -> HashSet<NodeId> {
    let mut out = HashSet::new();
    for &root in roots {
        let Some(n) = doc.get(root) else { continue };
        if is_container(n.kind()) {
            out.insert(root);
        }
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            let mut by_key: HashMap<String, Vec<NodeId>> = HashMap::new();
            for &c in n.children() {
                let Some(child) = doc.get(c) else { continue };
                if !is_container(child.kind()) {
                    continue;
                }
                stack.push(c);
                let key = match main_of(doc, c) {
                    Some(main) => format!("main:{main:?}"),
                    None => format!(
                        "{}:{}",
                        stem(child.name()),
                        matches!(child.kind(), NodeKind::Group)
                    ),
                };
                by_key.entry(key).or_default().push(c);
            }
            out.extend(by_key.into_values().filter(|v| v.len() >= 2).flatten());
        }
    }
    out
}

/// The nearest record frame above `id`, or `None` outside every one.
fn record_of(doc: &Document, id: NodeId, roots: &HashSet<NodeId>) -> Option<NodeId> {
    let mut at = doc.get(id).and_then(|n| n.parent());
    while let Some(a) = at {
        if roots.contains(&a) {
            return Some(a);
        }
        at = doc.get(a).and_then(|n| n.parent());
    }
    None
}

/// A fill's rows and records before any value is drawn, with each record's
/// seed: a hash of what it is (its frame, or its loose layers' parent or
/// layer), so a record draws the same values wherever it falls in the list.
struct Layout {
    rows: Vec<Row>,
    records: Vec<Record>,
    seeds: Vec<u64>,
    locked: usize,
}

fn layout(doc: &Document, roots: &[NodeId], opts: &Options) -> Layout {
    let (targets, locked) = gather(doc, roots);
    let label_like: HashSet<NodeId> = targets
        .iter()
        .filter(|t| t.label_like)
        .map(|t| t.node)
        .collect();
    // Each layer's kind: the one chosen for it, else the box's, else its own.
    let mut rows: Vec<Row> = targets
        .into_iter()
        .filter(|t| !t.shape || opts.avatars.is_some())
        .map(|t| {
            let kind = match opts.kinds.get(&t.node) {
                Some(chosen) => *chosen,
                None if t.shape => t.read,
                None => opts.content.filter(|k| k.is_text()).or(t.read),
            };
            Row {
                node: t.node,
                record: 0,
                kind,
                read: t.read,
                shape: t.shape,
                value: Value::Kept,
                fits: true,
            }
        })
        .collect();
    let frames = match opts.records {
        true => record_roots(doc, roots),
        false => HashSet::new(),
    };
    let parent = |id: NodeId| doc.get(id).and_then(|n| n.parent());

    // A layer named only by its text is a label, read as nothing, where another
    // layer shows the same kind beside it (a field's label over its value) or in
    // a record inside its own (a table's header over its rows).
    let within = |inner: NodeId, outer: Option<NodeId>| {
        let Some(outer) = outer else { return true };
        let mut at = parent(inner);
        while let Some(a) = at {
            if a == outer {
                return true;
            }
            at = parent(a);
        }
        false
    };
    let labels: HashSet<NodeId> = rows
        .iter()
        .filter(|a| label_like.contains(&a.node))
        .filter(|a| {
            let own = record_of(doc, a.node, &frames);
            rows.iter().any(|b| {
                let beside = parent(b.node) == parent(a.node) && !label_like.contains(&b.node);
                let below = record_of(doc, b.node, &frames)
                    .is_some_and(|theirs| Some(theirs) != own && within(theirs, own));
                b.node != a.node && b.read == a.read && (beside || below)
            })
        })
        .map(|a| a.node)
        .collect();
    for row in rows.iter_mut().filter(|r| labels.contains(&r.node)) {
        row.read = None;
        if !opts.kinds.contains_key(&row.node) && opts.content.is_none() {
            row.kind = None;
        }
    }

    // Records: a frame's layers draw together; loose layers under one parent
    // draw together unless two of them want the same kind, when each is its own.
    let mut records: Vec<Record> = Vec::new();
    let mut seeds: Vec<u64> = Vec::new();
    let mut index: HashMap<(Option<NodeId>, Option<NodeId>), usize> = HashMap::new();
    let mut repeated: HashSet<Option<NodeId>> = HashSet::new();
    if opts.records {
        let mut seen: HashSet<(Option<NodeId>, Kind)> = HashSet::new();
        for r in rows
            .iter()
            .filter(|r| record_of(doc, r.node, &frames).is_none())
        {
            if let Some(k) = r.kind
                && !seen.insert((parent(r.node), k))
            {
                repeated.insert(parent(r.node));
            }
        }
    }
    for row in &mut rows {
        let key = match record_of(doc, row.node, &frames) {
            Some(frame) => (Some(frame), None),
            None if opts.records && !repeated.contains(&parent(row.node)) => {
                (None, parent(row.node))
            }
            None => (None, Some(row.node)),
        };
        row.record = *index.entry(key).or_insert_with(|| {
            records.push(Record { root: key.0 });
            seeds.push(hash_str(&format!("{key:?}")));
            records.len() - 1
        });
    }
    Layout {
        rows,
        records,
        seeds,
        locked,
    }
}

/// What [`plan`] draws a record with, beside the record itself.
struct Draw<'a> {
    doc: &'a Document,
    opts: &'a Options,
    locale: &'a Locale,
    /// The domain the fill's emails share when a record names no company.
    shared: &'a str,
}

/// One record's values, by row index, and the list entries they took.
struct RecordDraw {
    values: Vec<(usize, Value, bool)>,
    picks: Picks,
}

fn add_picks(taken: &mut Taken, picks: &Picks) {
    for (list, entry) in picks {
        taken.entry(list).or_default().insert(entry.clone());
    }
}

/// The values of the record whose rows are `members`, seeded by `seed`,
/// passing over what `taken` holds. Unless `redrawn`, every row draws as if it
/// had never been redrawn.
fn draw_record(
    d: &Draw<'_>,
    rows: &[Row],
    members: &[usize],
    seed: u64,
    redrawn: bool,
    taken: &Taken,
) -> RecordDraw {
    let (doc, opts, locale) = (d.doc, d.opts, d.locale);
    let redraws = |id: NodeId| match redrawn {
        true => opts.redraws.get(&id).copied().unwrap_or(0),
        false => 0,
    };
    let uses = |e: Entity| {
        members
            .iter()
            .any(|i| rows[*i].kind.and_then(entity_of) == Some(e))
    };
    let salt = |e: Entity| -> u64 {
        members
            .iter()
            .filter(|i| rows[**i].kind.and_then(entity_of) == Some(e))
            .map(|i| redraws(rows[*i].node))
            .sum()
    };
    let rooms: Vec<Room> = members
        .iter()
        .map(|i| {
            doc.get(rows[*i].node)
                .map_or(Room::Free, |n| room(n.kind()))
        })
        .collect();
    let entity_rng =
        |e: Entity, attempt: u64| Rng::new(&[opts.seed, seed, e as u64 + 1, salt(e), attempt]);
    let mut drawn = Drawn {
        locale,
        person: Person::draw(locale, &mut entity_rng(Entity::Person, 0), taken),
        place: Place::draw(locale, &mut entity_rng(Entity::Place, 0), taken),
        firm: Firm::draw(locale, &mut entity_rng(Entity::Firm, 0), taken),
        domain: d.shared,
        own_firm: members.iter().any(|i| rows[*i].kind == Some(Kind::Company)),
        example_emails: opts.example_emails,
        taken,
    };
    // Each entity is redrawn until every row drawn from it fits its line.
    for e in [Entity::Firm, Entity::Person, Entity::Place] {
        for attempt in 0..ATTEMPTS {
            let mut rng = entity_rng(e, attempt);
            match e {
                Entity::Person => drawn.person = Person::draw(locale, &mut rng, taken),
                Entity::Place => drawn.place = Place::draw(locale, &mut rng, taken),
                Entity::Firm => drawn.firm = Firm::draw(locale, &mut rng, taken),
            }
            let all_fit = members.iter().zip(&rooms).all(|(i, room)| {
                let row = &rows[*i];
                match row.kind.filter(|k| entity_of(*k) == Some(e) && k.is_text()) {
                    Some(k) => doc.get(row.node).is_none_or(|n| {
                        // The stream and form the row's own draw below takes.
                        let mut rng = Rng::new(&[
                            opts.seed,
                            hash_str(&format!("{:?}", row.node)),
                            redraws(row.node),
                            0,
                        ]);
                        let text = drawn.text(k, n.name(), &mut rng, &mut Picks::new());
                        fits(n.kind(), *room, &for_room(text, *room))
                    }),
                    None => true,
                }
            });
            if all_fit {
                break;
            }
            if attempt + 1 == ATTEMPTS {
                let mut rng = entity_rng(e, 0);
                match e {
                    Entity::Person => drawn.person = Person::draw(locale, &mut rng, taken),
                    Entity::Place => drawn.place = Place::draw(locale, &mut rng, taken),
                    Entity::Firm => drawn.firm = Firm::draw(locale, &mut rng, taken),
                }
            }
        }
    }
    // The person always counts as taken; a place or company only where a row
    // shows it, so a list of names leaves the cities to the lists that show them.
    let mut picks = drawn.person.picks();
    if uses(Entity::Place) {
        picks.extend(drawn.place.picks());
    }
    if uses(Entity::Firm) {
        picks.extend(drawn.firm.picks());
    }
    let mut values = Vec::new();
    for (i, room) in members.iter().zip(rooms) {
        let row = &rows[*i];
        let Some(kind) = row.kind else { continue };
        let Some(node) = doc.get(row.node) else {
            continue;
        };
        let node_seed = hash_str(&format!("{:?}", row.node));
        let (value, ok) = if kind == Kind::Avatar {
            match row.shape {
                true => (
                    Value::Avatar(Avatar {
                        name: drawn.person.full(),
                        initials: drawn.person.initials(),
                        style: opts.avatars.unwrap_or_default(),
                    }),
                    true,
                ),
                false => (Value::Kept, true),
            }
        } else if row.shape {
            (Value::Kept, true)
        } else {
            let mut best: Option<(String, bool, Picks)> = None;
            for attempt in 0..ATTEMPTS {
                let mut rng = Rng::new(&[opts.seed, node_seed, redraws(row.node), attempt]);
                let mut trial = picks.clone();
                let text = match kind {
                    Kind::Paragraph => paragraph(&drawn, node.kind(), room, &mut rng, &mut trial),
                    _ => for_room(drawn.text(kind, node.name(), &mut rng, &mut trial), room),
                };
                let ok = fits(node.kind(), room, &text);
                if best.is_none() || ok {
                    best = Some((text, ok, trial));
                }
                // A value drawn from the record, or the locale's one country,
                // does not change with another try.
                if ok || entity_of(kind).is_some() || kind == Kind::Country {
                    break;
                }
            }
            let (text, ok, trial) = best.unwrap_or_default();
            picks = trial;
            (Value::Text(text), ok)
        };
        values.push((*i, value, ok));
    }
    RecordDraw { values, picks }
}

/// The fill of the layers under `roots` that `opts` asks for. Read-only: the
/// edit is [`Plan::transaction`].
///
/// Every record is drawn first as if nothing were redrawn, each passing over
/// the names, cities, jobs and lines the records before it took. A record with
/// a redrawn row is then drawn again, passing over every other record's values,
/// so a redraw changes its own record and no other.
pub fn plan(doc: &Document, roots: &[NodeId], opts: &Options) -> Plan {
    let Some(locale) = locale_for(&opts.locale).or_else(|| locales().first()) else {
        return Plan::default();
    };
    let Layout {
        mut rows,
        records,
        seeds,
        locked,
    } = layout(doc, roots, opts);
    let shared = Firm::draw(locale, &mut Rng::new(&[opts.seed, 0xF1]), &Taken::new());
    let d = Draw {
        doc,
        opts,
        locale,
        shared: &shared.domain,
    };
    let members: Vec<Vec<usize>> = (0..records.len())
        .map(|r| (0..rows.len()).filter(|i| rows[*i].record == r).collect())
        .collect();
    let mut taken = Taken::new();
    let mut draws: Vec<RecordDraw> = Vec::with_capacity(records.len());
    for r in 0..records.len() {
        let draw = draw_record(&d, &rows, &members[r], seeds[r], false, &taken);
        add_picks(&mut taken, &draw.picks);
        draws.push(draw);
    }
    let redrawn = |r: usize| {
        members[r]
            .iter()
            .any(|i| opts.redraws.get(&rows[*i].node).is_some_and(|n| *n > 0))
    };
    let mut again = Picks::new();
    for r in (0..records.len()).filter(|r| redrawn(*r)) {
        let mut others = Taken::new();
        for (o, draw) in draws.iter().enumerate() {
            if o != r {
                add_picks(&mut others, &draw.picks);
            }
        }
        add_picks(&mut others, &again);
        let draw = draw_record(&d, &rows, &members[r], seeds[r], true, &others);
        again.extend(draw.picks.iter().cloned());
        draws[r] = draw;
    }
    for draw in draws {
        for (i, value, ok) in draw.values {
            rows[i].value = value;
            rows[i].fits = ok;
        }
    }
    Plan {
        rows,
        records,
        locked,
    }
}

/// The rows and records [`plan`] would make, with no value drawn: a row that
/// would change holds an empty text. For counts of a choice not taken, which a
/// whole plan's text measuring would make dear.
pub fn outline(doc: &Document, roots: &[NodeId], opts: &Options) -> Plan {
    let Layout {
        mut rows,
        records,
        locked,
        ..
    } = layout(doc, roots, opts);
    for row in &mut rows {
        if row.kind.is_some_and(|k| k.is_text() != row.shape) {
            row.value = Value::Text(String::new());
        }
    }
    Plan {
        rows,
        records,
        locked,
    }
}

/// A paragraph for `room`: sentences added while a fixed box has space, up to
/// the length a growing box has now, or one where the width is the text's own.
fn paragraph(
    drawn: &Drawn<'_>,
    kind: &NodeKind,
    room: Room,
    rng: &mut Rng,
    picks: &mut Picks,
) -> String {
    let mut text = drawn.sentence(rng, picks);
    let mut used = HashSet::from([text.clone()]);
    for _ in 0..24 {
        let next = drawn.sentence(rng, picks);
        // The bank is spent: what is left repeats this paragraph.
        if !used.insert(next.clone()) {
            break;
        }
        let longer = format!("{text} {next}");
        match room {
            Room::Area(..) if fits(kind, room, &longer) => text = longer,
            Room::Wrap(chars) if text.chars().count() < chars => text = longer,
            _ => break,
        }
    }
    text
}

/// Whether a phone layer's name asks for a mobile number.
fn is_mobile(name: &str) -> bool {
    const MOBILE: [&str; 7] = [
        "mobile",
        "cell",
        "handy",
        "móvil",
        "cellulare",
        "celular",
        "mobiel",
    ];
    words(name).iter().any(|w| MOBILE.contains(&w.as_str()))
}

/// The years a date layer's name asks for: a birthday's decades back, a
/// creation or update's recent past, else the years around now.
fn date_years(name: &str) -> (u64, u64) {
    const BIRTH: [&str; 12] = [
        "birthday",
        "birth",
        "born",
        "dob",
        "geburtstag",
        "geburtsdatum",
        "naissance",
        "nacimiento",
        "nascita",
        "nascimento",
        "geboortedatum",
        "verjaardag",
    ];
    const PAST: [&str; 9] = [
        "created",
        "updated",
        "joined",
        "since",
        "published",
        "posted",
        "modified",
        "erstellt",
        "aktualisiert",
    ];
    let ws = words(name);
    let any = |list: &[&str]| list.iter().any(|w| ws.iter().any(|x| x == w));
    match () {
        _ if any(&BIRTH) => (1965, 2004),
        _ if any(&PAST) => (2022, 2025),
        _ => (2025, 2027),
    }
}

/// A value laid out for its room: a full address on one line reads with commas
/// where its lines broke.
fn for_room(text: String, room: Room) -> String {
    match room {
        Room::Line(_) if text.contains('\n') => text.replace('\n', ", "),
        _ => text,
    }
}

/// Whether anything under `roots` can be filled: a text layer, or a shape named
/// as an avatar or showing a picture, that is shown and not locked. Stops at
/// the first, since the doors ask every frame a menu is open.
pub fn fillable(doc: &Document, roots: &[NodeId]) -> bool {
    let mut stack: Vec<NodeId> = roots.to_vec();
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !doc.shown(id) {
            continue;
        }
        let target = match n.kind() {
            NodeKind::Text { .. } => n.text_vars()[crate::variable::TEXT_CONTENT].is_none(),
            _ => is_shape_target(n),
        };
        if target && !is_effectively_locked(doc, id) {
            return true;
        }
        stack.extend(n.children());
    }
    false
}

/// The locale the text layers under `roots` that set a Language agree on, or
/// `None` when they disagree, one names a locale not shipped, or none sets one.
pub fn agreed_locale(doc: &Document, roots: &[NodeId]) -> Option<&'static Locale> {
    let (targets, _) = gather(doc, roots);
    let mut agreed: Option<&'static Locale> = None;
    for t in targets.iter().filter(|t| !t.shape) {
        let NodeKind::Text { style, .. } = doc.get(t.node)?.kind() else {
            continue;
        };
        // A text without a Language says nothing; one in a locale not shipped
        // disagrees with every shipped one.
        let Some(tag) = style.locale.as_deref() else {
            continue;
        };
        let l = locale_for(tag)?;
        if agreed.is_some_and(|a| a.tag != l.tag) {
            return None;
        }
        agreed = Some(l);
    }
    agreed
}

/// A sample of `kind` in `locale`, for the kind list's faint tail.
pub fn sample(kind: Kind, locale: &str) -> String {
    let Some(locale) = locale_for(locale) else {
        return String::new();
    };
    let mut rng = Rng::new(&[1, kind as u64]);
    let taken = Taken::new();
    let shared = Firm::draw(locale, &mut rng, &taken);
    let drawn = Drawn {
        locale,
        person: Person::draw(locale, &mut rng, &taken),
        place: Place::draw(locale, &mut rng, &taken),
        firm: Firm::draw(locale, &mut rng, &taken),
        domain: &shared.domain,
        own_firm: false,
        example_emails: false,
        taken: &taken,
    };
    match kind {
        Kind::Avatar => drawn.person.initials(),
        Kind::Address => drawn.place.address(locale).replace('\n', ", "),
        _ => drawn.text(kind, "", &mut rng, &mut Picks::new()),
    }
}
