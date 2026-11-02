//! Which fonts draw the characters a text's own family cannot (§5.4a).
//!
//! Core shapes only against fonts it has been handed, so nothing falls back to the
//! system's fonts: a character missing from the named family and from the bundled
//! Inter draws as an empty box unless the font stack names a family that has it.
//! This module says which families those are, and the app loads them.
//!
//! A web font arrives in **subsets** — a file per script, named as the Fontsource
//! (and Google Fonts) CSS names them. A subset face is registered under
//! [`subset_alias`], not under its family's own name: one family holding two faces
//! of the same weight and style keeps only one of them in the match, so a Cyrillic
//! file registered as plain `Roboto` would shadow the Latin one or be shadowed by it.

use std::collections::BTreeSet;

/// A web-font subset past the default `latin` one, by the characters it holds.
///
/// The ranges follow the subsets' published `unicode-range`s, reduced to the
/// blocks that identify a script (combining marks and punctuation that several
/// subsets share are left to whichever face the text already uses).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Subset {
    LatinExt,
    Vietnamese,
    Cyrillic,
    CyrillicExt,
    Greek,
    GreekExt,
    Arabic,
    Hebrew,
    Devanagari,
    Bengali,
    Tamil,
    Thai,
    Armenian,
    Georgian,
    Ethiopic,
    Khmer,
    Japanese,
    Korean,
    ChineseSimplified,
    ChineseTraditional,
}

impl Subset {
    /// Every subset. Kept by hand: a new variant is not a compile error here.
    pub const ALL: [Subset; 20] = [
        Subset::LatinExt,
        Subset::Vietnamese,
        Subset::Cyrillic,
        Subset::CyrillicExt,
        Subset::Greek,
        Subset::GreekExt,
        Subset::Arabic,
        Subset::Hebrew,
        Subset::Devanagari,
        Subset::Bengali,
        Subset::Tamil,
        Subset::Thai,
        Subset::Armenian,
        Subset::Georgian,
        Subset::Ethiopic,
        Subset::Khmer,
        Subset::Japanese,
        Subset::Korean,
        Subset::ChineseSimplified,
        Subset::ChineseTraditional,
    ];

    /// The subset's name in a Fontsource file name and its `subsets` list.
    pub fn name(self) -> &'static str {
        match self {
            Subset::LatinExt => "latin-ext",
            Subset::Vietnamese => "vietnamese",
            Subset::Cyrillic => "cyrillic",
            Subset::CyrillicExt => "cyrillic-ext",
            Subset::Greek => "greek",
            Subset::GreekExt => "greek-ext",
            Subset::Arabic => "arabic",
            Subset::Hebrew => "hebrew",
            Subset::Devanagari => "devanagari",
            Subset::Bengali => "bengali",
            Subset::Tamil => "tamil",
            Subset::Thai => "thai",
            Subset::Armenian => "armenian",
            Subset::Georgian => "georgian",
            Subset::Ethiopic => "ethiopic",
            Subset::Khmer => "khmer",
            Subset::Japanese => "japanese",
            Subset::Korean => "korean",
            Subset::ChineseSimplified => "chinese-simplified",
            Subset::ChineseTraditional => "chinese-traditional",
        }
    }

    /// Whether the bundled Inter draws this subset, so no fallback family is needed
    /// for it (a named web family still fetches its own file for it).
    pub fn inter_covers(self) -> bool {
        matches!(
            self,
            Subset::LatinExt
                | Subset::Vietnamese
                | Subset::Cyrillic
                | Subset::CyrillicExt
                | Subset::Greek
                | Subset::GreekExt
        )
    }

    /// Whether one file of this subset runs to megabytes, so only a regular and a
    /// bold are fetched rather than every weight.
    pub fn is_heavy(self) -> bool {
        matches!(
            self,
            Subset::Japanese
                | Subset::Korean
                | Subset::ChineseSimplified
                | Subset::ChineseTraditional
        )
    }

    /// The families that draw this subset when the text's own family does not, in
    /// the order they are preferred: the Noto family first (installed, or fetched),
    /// then the faces Windows and macOS ship for the script, for a machine with web
    /// fonts off. Empty for what Inter covers.
    pub fn fallbacks(self) -> &'static [&'static str] {
        match self {
            Subset::Arabic => &["Noto Sans Arabic", "Segoe UI", "Geeza Pro", "Tahoma"],
            Subset::Hebrew => &["Noto Sans Hebrew", "Segoe UI", "Arial Hebrew", "Tahoma"],
            Subset::Devanagari => &["Noto Sans Devanagari", "Nirmala UI", "Kohinoor Devanagari"],
            Subset::Bengali => &["Noto Sans Bengali", "Nirmala UI", "Kohinoor Bangla"],
            Subset::Tamil => &["Noto Sans Tamil", "Nirmala UI", "Tamil Sangam MN"],
            Subset::Thai => &["Noto Sans Thai", "Leelawadee UI", "Thonburi"],
            Subset::Armenian => &["Noto Sans Armenian", "Segoe UI", "Mshtakan"],
            Subset::Georgian => &["Noto Sans Georgian", "Segoe UI", "Sylfaen"],
            Subset::Ethiopic => &["Noto Sans Ethiopic", "Ebrima", "Kefa"],
            Subset::Khmer => &["Noto Sans Khmer", "Leelawadee UI", "Khmer Sangam MN"],
            Subset::Japanese => &[
                "Noto Sans JP",
                "Noto Sans CJK JP",
                "Yu Gothic",
                "Hiragino Sans",
                "Meiryo",
            ],
            Subset::Korean => &[
                "Noto Sans KR",
                "Noto Sans CJK KR",
                "Malgun Gothic",
                "Apple SD Gothic Neo",
            ],
            Subset::ChineseSimplified => &[
                "Noto Sans SC",
                "Noto Sans CJK SC",
                "Microsoft YaHei",
                "PingFang SC",
            ],
            Subset::ChineseTraditional => &[
                "Noto Sans TC",
                "Noto Sans CJK TC",
                "Microsoft JhengHei",
                "PingFang TC",
            ],
            _ => &[],
        }
    }
}

/// The name a face of `family` holding only `subset` is registered under (see the
/// module docs). Never a real family's name, so it cannot collide with one.
pub fn subset_alias(family: &str, subset: Subset) -> String {
    format!("{family} \u{2039}{}\u{203a}", subset.name())
}

/// Every subset `text`'s characters fall in, Han ideographs assigned by `locale`
/// (a BCP 47 tag) and, without one, by the kana or hangul beside them.
pub fn subsets_in(text: &str, locale: Option<&str>) -> BTreeSet<Subset> {
    let (mut out, han) = scan(text);
    if han {
        out.insert(han_subset(locale, &out));
    }
    out
}

/// The subsets `text`'s characters other than Han fall in, and whether it holds
/// Han, whose subset depends on a locale.
fn scan(text: &str) -> (BTreeSet<Subset>, bool) {
    let mut out = BTreeSet::new();
    let mut han = false;
    // ASCII is the common case and in no subset past `latin`.
    for c in text.chars().filter(|c| !c.is_ascii()) {
        match class_of(c) {
            Some(Class::Subset(s)) => {
                out.insert(s);
            }
            Some(Class::Both(a, b)) => {
                out.insert(a);
                out.insert(b);
            }
            Some(Class::Han) => han = true,
            None => {}
        }
    }
    (out, han)
}

/// [`subsets_in`] for a text whose runs carry locales of their own: the Han
/// subset is chosen per run, by the run's locale and the whole text's kana and
/// hangul, so a `ja` span in a node with none draws Japanese forms.
pub fn subsets_in_runs<'a>(
    text: &str,
    runs: impl IntoIterator<Item = (std::ops::Range<usize>, Option<&'a str>)>,
) -> BTreeSet<Subset> {
    let (mut out, han) = scan(text);
    if han {
        let others = out.clone();
        for (range, locale) in runs {
            if text.get(range).is_some_and(|piece| scan(piece).1) {
                out.insert(han_subset(locale, &others));
            }
        }
    }
    out
}

/// Every subset the text layers on `doc`'s pages fall in — their content, and a
/// bound content variable's value in every mode — so the fonts a page switch or a
/// mode switch needs are already loading.
pub fn subsets_of(doc: &crate::Document) -> BTreeSet<Subset> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<crate::NodeId> = doc.page_ids().collect();
    while let Some(id) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        if let crate::node::NodeKind::Text {
            content,
            style,
            spans,
            ..
        } = node.kind()
        {
            let locale = style.locale.as_deref();
            let runs = spans.runs(style, content.len());
            out.extend(subsets_in_runs(
                content,
                runs.iter().map(|(r, s)| (r.clone(), s.locale.as_deref())),
            ));
            // Binding the content clears the spans, so a bound value has the
            // node's locale only.
            if let Some(var) = node.text_vars()[crate::variable::TEXT_CONTENT] {
                for value in doc.variables().every_value(var) {
                    if let Some(s) = value.as_str() {
                        out.extend(subsets_in(s, locale));
                    }
                }
            }
        }
        stack.extend(node.children().iter().copied());
    }
    out
}

/// The subsets `text`'s characters other than Han fall in: what [`han_subset`]
/// reads beside a run's locale.
pub fn scripts_in(text: &str) -> BTreeSet<Subset> {
    scan(text).0
}

/// Which CJK subset Han ideographs in a run tagged `locale` are drawn from, in a
/// text whose other characters fall in `others` ([`scripts_in`]).
pub fn han_subset(locale: Option<&str>, others: &BTreeSet<Subset>) -> Subset {
    let tag = locale.unwrap_or("").to_ascii_lowercase();
    let lang = tag.split(['-', '_']).next().unwrap_or("");
    match lang {
        "ja" => return Subset::Japanese,
        "ko" => return Subset::Korean,
        "zh" => {
            let traditional = ["hant", "tw", "hk", "mo"]
                .iter()
                .any(|t| tag.split(['-', '_']).any(|p| p == *t));
            return if traditional {
                Subset::ChineseTraditional
            } else {
                Subset::ChineseSimplified
            };
        }
        _ => {}
    }
    if others.contains(&Subset::Japanese) {
        Subset::Japanese
    } else if others.contains(&Subset::Korean) {
        Subset::Korean
    } else {
        Subset::ChineseSimplified
    }
}

enum Class {
    Subset(Subset),
    /// A character two subsets carry, such as Vietnamese's Ă, also in `latin-ext`.
    Both(Subset, Subset),
    /// A Han ideograph or CJK punctuation, whose subset depends on the language.
    Han,
}

fn class_of(c: char) -> Option<Class> {
    use Subset::*;
    let s = |s| Some(Class::Subset(s));
    match c as u32 {
        0x0102..=0x0103
        | 0x0110..=0x0111
        | 0x0128..=0x0129
        | 0x0168..=0x0169
        | 0x01A0..=0x01A1
        | 0x01AF..=0x01B0 => Some(Class::Both(LatinExt, Vietnamese)),
        0x1EA0..=0x1EF1 | 0x20AB => s(Vietnamese),
        0x0100..=0x024F | 0x1D00..=0x1DBF | 0x1E00..=0x1E9F | 0x1EF2..=0x1EFF => s(LatinExt),
        0x2C60..=0x2C7F | 0xA720..=0xA7FF => s(LatinExt),
        0x0400..=0x045F | 0x0490..=0x0491 | 0x04B0..=0x04B1 | 0x2116 => s(Cyrillic),
        0x0460..=0x052F | 0x1C80..=0x1C8A | 0x2DE0..=0x2DFF | 0xA640..=0xA69F => s(CyrillicExt),
        0x0370..=0x03FF => s(Greek),
        0x1F00..=0x1FFF => s(GreekExt),
        0x0530..=0x058F | 0xFB13..=0xFB17 => s(Armenian),
        0x0590..=0x05FF | 0xFB1D..=0xFB4F => s(Hebrew),
        0x0600..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF => s(Arabic),
        0xFB50..=0xFDFF | 0xFE70..=0xFEFC => s(Arabic),
        0x0900..=0x097F | 0x1CD0..=0x1CF9 | 0xA8E0..=0xA8FF => s(Devanagari),
        0x0980..=0x09FF => s(Bengali),
        0x0B80..=0x0BFF => s(Tamil),
        0x0E00..=0x0E7F => s(Thai),
        0x10A0..=0x10FF | 0x1C90..=0x1CBF | 0x2D00..=0x2D2F => s(Georgian),
        0x1200..=0x139F | 0x2D80..=0x2DDF | 0xAB00..=0xAB2F => s(Ethiopic),
        0x1780..=0x17FF | 0x19E0..=0x19FF => s(Khmer),
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => s(Japanese),
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F => s(Korean),
        0xAC00..=0xD7FF | 0xFFA0..=0xFFDC => s(Korean),
        0x2E80..=0x2FDF | 0x3000..=0x303F | 0x3100..=0x312F | 0x31C0..=0x31EF => Some(Class::Han),
        0x3200..=0x33FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => Some(Class::Han),
        0xFE30..=0xFE4F | 0xFF00..=0xFF65 | 0x20000..=0x3134F => Some(Class::Han),
        _ => None,
    }
}

/// Open families that draw a proprietary or system-only one in its place, by
/// lowercase name, most faithful first.
///
/// Metric-compatible clones lead, every character as wide as the original's so
/// lines break where they did. A clone that only ships as an installable package
/// (Liberation, TeX Gyre, URW and the like) comes before the web one, so a machine
/// that has it needs no download. Look-alikes of other widths come last. The web
/// Caladea is a later cut whose widths differ from Cambria's (only the original
/// release matches), so Cambria is drawn in a look-alike unless that is installed.
pub static STAND_INS: &[(&str, &[&str])] = &[
    // Metric-compatible.
    ("arial", &["Liberation Sans", "Arimo"]),
    (
        "helvetica",
        &["Liberation Sans", "Nimbus Sans", "TeX Gyre Heros", "Arimo"],
    ),
    (
        "arial narrow",
        &["Liberation Sans Narrow", "Nimbus Sans Narrow"],
    ),
    (
        "helvetica narrow",
        &["Liberation Sans Narrow", "Nimbus Sans Narrow"],
    ),
    (
        "helvetica condensed",
        &["Liberation Sans Narrow", "TeX Gyre Heros Cn"],
    ),
    ("times new roman", &["Liberation Serif", "Tinos"]),
    (
        "times",
        &[
            "Liberation Serif",
            "Nimbus Roman",
            "TeX Gyre Termes",
            "Tinos",
        ],
    ),
    (
        "times roman",
        &[
            "Liberation Serif",
            "Nimbus Roman",
            "TeX Gyre Termes",
            "Tinos",
        ],
    ),
    (
        "courier new",
        &["Liberation Mono", "Cousine", "Courier Prime"],
    ),
    (
        "courier",
        &[
            "Liberation Mono",
            "Nimbus Mono PS",
            "TeX Gyre Cursor",
            "Cousine",
        ],
    ),
    ("georgia", &["Gelasio"]),
    ("calibri", &["Carlito"]),
    ("comic sans ms", &["Comic Relief", "Comic Neue"]),
    ("comic sans", &["Comic Relief", "Comic Neue"]),
    ("segoe ui", &["Selawik", "Arimo"]),
    ("tahoma", &["Wine Tahoma", "DejaVu Sans", "Arimo"]),
    ("consolas", &["DMCA Sans Serif", "Inconsolata"]),
    ("lucida console", &["DejaVu Sans Mono", "Cousine"]),
    ("microsoft sans serif", &["Arimo"]),
    ("ms sans serif", &["Arimo"]),
    (
        "century gothic",
        &["URW Gothic", "TeX Gyre Adventor", "Jost"],
    ),
    (
        "itc avant garde gothic",
        &["URW Gothic", "TeX Gyre Adventor", "Jost"],
    ),
    ("avant garde", &["URW Gothic", "TeX Gyre Adventor", "Jost"]),
    (
        "avant garde gothic",
        &["URW Gothic", "TeX Gyre Adventor", "Jost"],
    ),
    ("palatino", &["TeX Gyre Pagella", "P052", "Tinos"]),
    ("palatino linotype", &["TeX Gyre Pagella", "P052", "Tinos"]),
    ("book antiqua", &["TeX Gyre Pagella", "P052", "Tinos"]),
    ("bookman", &["TeX Gyre Bonum", "URW Bookman", "Tinos"]),
    ("itc bookman", &["TeX Gyre Bonum", "URW Bookman", "Tinos"]),
    (
        "bookman old style",
        &["TeX Gyre Bonum", "URW Bookman", "Tinos"],
    ),
    (
        "century schoolbook",
        &["TeX Gyre Schola", "C059", "Old Standard TT"],
    ),
    (
        "new century schoolbook",
        &["TeX Gyre Schola", "C059", "Old Standard TT"],
    ),
    ("itc zapf chancery", &["TeX Gyre Chorus", "Z003"]),
    ("zapf chancery", &["TeX Gyre Chorus", "Z003"]),
    ("monotype corsiva", &["TeX Gyre Chorus", "Z003"]),
    // Look-alikes: system faces. Cambria's is metric only in Caladea's first release.
    ("cambria", &["Caladea"]),
    ("sf pro", &["Inter"]),
    ("sf pro text", &["Inter"]),
    ("sf pro display", &["Inter"]),
    ("san francisco", &["Inter"]),
    ("helvetica neue", &["Inter"]),
    ("sf mono", &["JetBrains Mono", "Source Code Pro"]),
    ("menlo", &["DejaVu Sans Mono"]),
    ("monaco", &["Cousine"]),
    ("new york", &["Tinos"]),
    ("verdana", &["DejaVu Sans", "Arimo"]),
    ("lucida grande", &["DejaVu Sans", "Arimo"]),
    ("lucida sans", &["DejaVu Sans", "Arimo"]),
    ("lucida sans unicode", &["DejaVu Sans", "Arimo"]),
    ("trebuchet ms", &["Arimo"]),
    ("impact", &["Anton", "Oswald"]),
    // Look-alikes: commercial sans.
    ("proxima nova", &["Montserrat", "Figtree"]),
    ("gotham", &["Montserrat"]),
    ("gotham rounded", &["Nunito"]),
    ("avenir", &["Nunito Sans"]),
    ("avenir next", &["Nunito Sans"]),
    ("futura", &["Jost", "League Spartan"]),
    ("futura pt", &["Jost", "League Spartan"]),
    ("circular", &["Plus Jakarta Sans", "Figtree", "DM Sans"]),
    ("circular std", &["Plus Jakarta Sans", "Figtree", "DM Sans"]),
    ("apercu", &["Hanken Grotesk"]),
    ("din", &["Gidole", "Barlow"]),
    ("ff din", &["Gidole", "Barlow"]),
    ("din pro", &["Gidole", "Barlow"]),
    ("din next", &["Gidole", "Barlow"]),
    ("din 1451", &["Gidole", "Barlow"]),
    ("brandon grotesque", &["Lato"]),
    ("museo sans", &["Nunito Sans"]),
    ("sofia pro", &["Montserrat"]),
    ("frutiger", &["Hind"]),
    ("corbel", &["Hind"]),
    ("myriad", &["Source Sans 3", "PT Sans"]),
    ("myriad pro", &["Source Sans 3", "PT Sans"]),
    ("gill sans", &["Lato", "Cabin"]),
    ("gill sans mt", &["Lato", "Cabin"]),
    ("franklin gothic", &["Libre Franklin", "Public Sans"]),
    ("itc franklin gothic", &["Libre Franklin", "Public Sans"]),
    ("franklin gothic medium", &["Libre Franklin", "Public Sans"]),
    ("alternate gothic", &["League Gothic", "Oswald"]),
    ("franklin gothic condensed", &["League Gothic", "Oswald"]),
    // Look-alikes: commercial serif and display.
    ("garamond", &["EB Garamond", "Cormorant Garamond"]),
    ("adobe garamond pro", &["EB Garamond", "Cormorant Garamond"]),
    (
        "garamond premier pro",
        &["EB Garamond", "Cormorant Garamond"],
    ),
    ("itc garamond", &["EB Garamond", "Cormorant Garamond"]),
    ("minion", &["Crimson Pro", "Crimson Text"]),
    ("minion pro", &["Crimson Pro", "Crimson Text"]),
    ("baskerville", &["Libre Baskerville"]),
    ("itc new baskerville", &["Libre Baskerville"]),
    ("baskerville old face", &["Libre Baskerville"]),
    ("caslon", &["Libre Caslon Text"]),
    ("adobe caslon pro", &["Libre Caslon Text"]),
    ("big caslon", &["Libre Caslon Display"]),
    ("bodoni", &["Libre Bodoni", "Bodoni Moda"]),
    ("itc bodoni", &["Libre Bodoni", "Bodoni Moda"]),
    ("bodoni 72", &["Libre Bodoni", "Bodoni Moda"]),
    ("didot", &["GFS Didot", "Playfair Display"]),
    ("hfj didot", &["GFS Didot", "Playfair Display"]),
    ("linotype didot", &["GFS Didot", "Playfair Display"]),
    ("trajan", &["Cinzel"]),
    ("trajan pro", &["Cinzel"]),
    ("optima", &["Marcellus"]),
    ("rockwell", &["Rokkitt", "Arvo"]),
    ("clarendon", &["Ultra"]),
];

/// Edition suffixes a file's family name often carries past the typeface's own,
/// tried off the end when the whole name has no stand-in.
const EDITIONS: &[&str] = &[" std", " pro", " mt", " lt", " com"];

/// The open families that draw `family` in its place, most faithful first; empty
/// for a family with none. Case-insensitive, quotes and edition suffixes
/// ([`EDITIONS`]) ignored.
pub fn stand_ins(family: &str) -> &'static [&'static str] {
    let mut key = family
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_lowercase();
    loop {
        if let Some((_, found)) = STAND_INS.iter().find(|(k, _)| *k == key) {
            return found;
        }
        match EDITIONS.iter().find(|e| key.ends_with(**e)) {
            Some(e) => key.truncate(key.len() - e.len()),
            None => return &[],
        }
    }
}

/// The families a run set in `family` is shaped against, in order: the family,
/// its subset faces for `subsets`, its [`stand_ins`] with theirs, the bundled
/// `default` (Inter), then the fallbacks of every subset Inter lacks, each with
/// its own subset face. A name nothing registered is skipped by the shaper, so the
/// stack can name every candidate and let whichever the app loaded answer.
///
/// `han` is the subset the run's own Han is drawn from ([`han_subset`]): it
/// leads the CJK subsets, since the shaper takes the first family that has a
/// character and every CJK face has the ideographs.
pub fn font_stack(
    family: &str,
    subsets: &BTreeSet<Subset>,
    han: Option<Subset>,
    default: &str,
) -> Vec<String> {
    let own = han.filter(|h| subsets.contains(h));
    let subsets: Vec<Subset> = own
        .into_iter()
        .chain(subsets.iter().copied().filter(|s| Some(*s) != own))
        .collect();
    let mut out = vec![family.to_string()];
    out.extend(subsets.iter().map(|s| subset_alias(family, *s)));
    for stand_in in stand_ins(family) {
        out.push((*stand_in).to_string());
        out.extend(subsets.iter().map(|s| subset_alias(stand_in, *s)));
    }
    // Once: the family or a stand-in may already be it.
    if !out.iter().any(|f| f.eq_ignore_ascii_case(default)) {
        out.push(default.to_string());
    }
    for subset in subsets.iter().filter(|s| !s.inter_covers()) {
        for fallback in subset.fallbacks() {
            out.push((*fallback).to_string());
            out.push(subset_alias(fallback, *subset));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(subsets: &[Subset]) -> BTreeSet<Subset> {
        subsets.iter().copied().collect()
    }

    #[test]
    fn plain_latin_needs_no_subset_and_accents_need_latin_ext() {
        assert!(subsets_in("Hello, café — naïve", None).is_empty());
        assert_eq!(subsets_in("Łódź", None), set(&[Subset::LatinExt]));
    }

    #[test]
    fn vietnamese_letters_shared_with_latin_ext_ask_for_both() {
        assert_eq!(
            subsets_in("Ă", None),
            set(&[Subset::LatinExt, Subset::Vietnamese])
        );
        assert_eq!(subsets_in("Việt", None), set(&[Subset::Vietnamese]));
    }

    #[test]
    fn scripts_are_told_apart() {
        assert_eq!(subsets_in("Привет", None), set(&[Subset::Cyrillic]));
        assert_eq!(subsets_in("مرحبا", None), set(&[Subset::Arabic]));
        assert_eq!(subsets_in("שלום", None), set(&[Subset::Hebrew]));
        assert_eq!(subsets_in("नमस्ते", None), set(&[Subset::Devanagari]));
        assert_eq!(subsets_in("สวัสดี", None), set(&[Subset::Thai]));
        assert_eq!(subsets_in("안녕", None), set(&[Subset::Korean]));
    }

    /// Han in a span tagged `ja` asks for Japanese though the node has no locale,
    /// beside Simplified for Han outside it; the document's preload reads spans too.
    ///
    /// flip: `subsets_in_runs` choosing Han by the node's locale alone → fails at
    /// "the span's".
    #[test]
    fn han_follows_a_spans_own_locale() {
        assert_eq!(
            subsets_in_runs("漢字漢", [(0..6, Some("ja")), (6..9, None)]),
            set(&[Subset::ChineseSimplified, Subset::Japanese]),
            "the span's, and the node's beside it"
        );
        let mut ids = crate::IdSource::new(7);
        let root = ids.mint();
        let mut doc = crate::Document::new(root);
        let style = crate::TextStyle::default();
        let mut spans = crate::node::CharSpans::default();
        spans.set(0..6, crate::CharAttr::Locale(Some("ja".into())), &style);
        doc.apply(&crate::Transaction(vec![crate::Operation::CreateNode {
            id: ids.mint(),
            parent: root,
            index: 0,
            kind: crate::NodeKind::Text {
                content: "漢字".into(),
                style: Box::new(style),
                spans,
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: crate::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        assert_eq!(subsets_of(&doc), set(&[Subset::Japanese]), "the preload");
    }

    #[test]
    fn han_follows_the_locale_then_the_kana_beside_it() {
        assert_eq!(subsets_in("漢字", None), set(&[Subset::ChineseSimplified]));
        assert_eq!(subsets_in("漢字", Some("ja")), set(&[Subset::Japanese]));
        assert_eq!(
            subsets_in("漢字", Some("zh-Hant")),
            set(&[Subset::ChineseTraditional])
        );
        assert_eq!(
            subsets_in("漢字", Some("zh-TW")),
            set(&[Subset::ChineseTraditional])
        );
        assert_eq!(
            subsets_in("漢字", Some("zh-CN")),
            set(&[Subset::ChineseSimplified])
        );
        assert_eq!(subsets_in("ひらがなと漢字", None), set(&[Subset::Japanese]));
    }

    #[test]
    fn the_stack_names_the_family_then_inter_then_the_scripts_inter_lacks() {
        let stack = font_stack(
            "Roboto",
            &set(&[Subset::Cyrillic, Subset::Japanese]),
            None,
            "Inter",
        );
        assert_eq!(stack[0], "Roboto");
        assert_eq!(stack[1], subset_alias("Roboto", Subset::Cyrillic));
        assert_eq!(stack[2], subset_alias("Roboto", Subset::Japanese));
        assert_eq!(stack[3], "Inter");
        assert_eq!(stack[4], "Noto Sans JP");
        assert_eq!(stack[5], subset_alias("Noto Sans JP", Subset::Japanese));
        // Cyrillic is Inter's own, so no fallback family is named for it.
        assert!(
            !stack
                .iter()
                .any(|f| f.starts_with("Noto Sans ") && f.contains("cyrillic"))
        );
    }

    /// Each run's stack leads its CJK fallbacks with its own Han subset: untagged
    /// Han beside a `ja` span keeps Chinese forms, a `zh-Hant` span beside untagged
    /// Han takes Traditional ones.
    ///
    /// flip: `font_stack` ignoring `han` → fails at "a zh-Hant span". Its wiring
    /// through `text::push_style` is not pinned: seeing which face drew a run needs
    /// a CJK font, which the repository does not carry.
    #[test]
    fn a_runs_own_han_leads_its_stack() {
        let all = set(&[
            Subset::Japanese,
            Subset::ChineseSimplified,
            Subset::ChineseTraditional,
        ]);
        let first_cjk = |han: Subset| {
            font_stack("Inter", &all, Some(han), "Inter")
                .into_iter()
                .find(|f| f.starts_with("Noto Sans"))
                .expect("a CJK fallback")
        };
        let scripts = scripts_in("漢字漢字");
        assert_eq!(
            first_cjk(han_subset(Some("zh-Hant"), &scripts)),
            "Noto Sans TC",
            "a zh-Hant span"
        );
        assert_eq!(
            first_cjk(han_subset(None, &scripts)),
            "Noto Sans SC",
            "untagged Han beside a ja span"
        );
        assert_eq!(
            first_cjk(han_subset(Some("ja"), &scripts)),
            "Noto Sans JP",
            "the ja span"
        );
    }

    #[test]
    fn inter_is_named_once() {
        assert_eq!(
            font_stack("Inter", &BTreeSet::new(), None, "Inter"),
            vec!["Inter"]
        );
        assert_eq!(
            font_stack("SF Pro", &BTreeSet::new(), None, "Inter"),
            vec!["SF Pro", "Inter"]
        );
    }

    /// A proprietary family's stand-ins follow it, ahead of Inter, each with its
    /// subset faces.
    #[test]
    fn stand_ins_come_between_the_family_and_inter() {
        let stack = font_stack("Calibri", &set(&[Subset::Cyrillic]), None, "Inter");
        assert_eq!(
            stack,
            vec![
                "Calibri".to_string(),
                subset_alias("Calibri", Subset::Cyrillic),
                "Carlito".to_string(),
                subset_alias("Carlito", Subset::Cyrillic),
                "Inter".to_string(),
            ]
        );
    }

    #[test]
    fn a_stand_in_is_found_without_case_quotes_or_an_edition_suffix() {
        assert_eq!(stand_ins("Arial"), ["Liberation Sans", "Arimo"]);
        assert_eq!(
            stand_ins("  'TIMES NEW ROMAN' "),
            ["Liberation Serif", "Tinos"]
        );
        assert_eq!(stand_ins("Helvetica Neue LT Pro"), ["Inter"]);
        assert_eq!(stand_ins("Proxima Nova"), stand_ins("Proxima Nova Std"));
        assert!(stand_ins("Roboto").is_empty());
        // A suffix is only ever taken off the end.
        assert!(stand_ins("Pro Sans").is_empty());
    }

    /// Every key is lowercase and listed once, and no family stands in for itself.
    #[test]
    fn the_stand_in_table_is_well_formed() {
        let mut keys = BTreeSet::new();
        for (key, found) in STAND_INS {
            assert_eq!(*key, key.to_lowercase(), "{key}");
            assert!(keys.insert(*key), "{key} listed twice");
            assert!(!found.is_empty(), "{key}");
            assert!(
                !found.iter().any(|f| f.eq_ignore_ascii_case(key)),
                "{key} stands in for itself"
            );
        }
    }

    /// Exactly the subsets Inter lacks name fallback families.
    #[test]
    fn every_subset_inter_lacks_has_a_fallback_and_the_rest_have_none() {
        for s in Subset::ALL {
            assert_eq!(s.fallbacks().is_empty(), s.inter_covers(), "{s:?}");
        }
    }
}
