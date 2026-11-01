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
    if han {
        out.insert(han_subset(locale, &out));
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
        if let crate::node::NodeKind::Text { content, style, .. } = node.kind() {
            let locale = style.locale.as_deref();
            out.extend(subsets_in(content, locale));
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

/// Which CJK subset Han ideographs are drawn from.
fn han_subset(locale: Option<&str>, others: &BTreeSet<Subset>) -> Subset {
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

/// The families a run set in `family` is shaped against, in order: the family,
/// its subset faces for `subsets`, the bundled `default` (Inter), then the
/// fallbacks of every subset Inter lacks, each with its own subset face. A name
/// nothing registered is skipped by the shaper, so the stack can name every
/// candidate and let whichever the app loaded answer.
pub fn font_stack(family: &str, subsets: &BTreeSet<Subset>, default: &str) -> Vec<String> {
    let mut out = vec![family.to_string()];
    out.extend(subsets.iter().map(|s| subset_alias(family, *s)));
    if !family.eq_ignore_ascii_case(default) {
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

    #[test]
    fn inter_is_named_once() {
        assert_eq!(
            font_stack("Inter", &BTreeSet::new(), "Inter"),
            vec!["Inter"]
        );
    }

    /// Exactly the subsets Inter lacks name fallback families.
    #[test]
    fn every_subset_inter_lacks_has_a_fallback_and_the_rest_have_none() {
        for s in Subset::ALL {
            assert_eq!(s.fallbacks().is_empty(), s.inter_covers(), "{s:?}");
        }
    }
}
