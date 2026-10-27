//! Batch rename's text rule: a find, a template and a number, applied to one
//! layer's name. Which layers it runs over, and in which order they are numbered,
//! is the app's (`batch_rename`).
//!
//! A template is plain text in which `[Name]`, `[Number]` and `[Group k]` are
//! tokens. An empty find replaces the whole name and `[Name]` is that name; a
//! filled one replaces every match, and `[Name]` is the matched text.

use regex::{Regex, RegexBuilder};
use std::fmt;
use std::ops::Range;

/// One piece of a template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    /// The layer's name, or the matched text when Find is filled.
    Name,
    /// The layer's place in the run, formatted by [`Numbering`].
    Number,
    /// A bracketed group of a pattern, counted from 1.
    Group(usize),
}

/// What *Rename to* holds: text and tokens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Template(pub Vec<Piece>);

/// The tokens in `src`, with their byte ranges. `[Name]`, `[Number]` and
/// `[Group k]` are tokens in any case, and so are the typed short forms `[n]` and
/// `[k]`; any other bracket is text.
pub fn tokens(src: &str) -> Vec<(Range<usize>, Piece)> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(open) = src[at..].find('[').map(|i| at + i) {
        let Some(close) = src[open..].find(']').map(|i| open + i) else {
            break;
        };
        // A `[` inside the brackets starts again from it: `[[name]` is `[` then a token.
        let inner = &src[open + 1..close];
        if let Some(i) = inner.rfind('[') {
            at = open + 1 + i;
            continue;
        }
        if let Some(piece) = token(inner) {
            out.push((open..close + 1, piece));
        }
        at = close + 1;
    }
    out
}

fn token(inner: &str) -> Option<Piece> {
    let word = inner.trim().to_lowercase();
    match word.as_str() {
        "name" => return Some(Piece::Name),
        "n" | "number" => return Some(Piece::Number),
        _ => {}
    }
    let digits = word.strip_prefix("group").map_or(word.as_str(), str::trim);
    match digits.parse::<usize>() {
        Ok(k) if k > 0 && digits.bytes().all(|b| b.is_ascii_digit()) => Some(Piece::Group(k)),
        _ => None,
    }
}

impl Template {
    pub fn parse(src: &str) -> Self {
        let mut pieces = Vec::new();
        let mut at = 0;
        for (range, piece) in tokens(src) {
            if range.start > at {
                pieces.push(Piece::Text(src[at..range.start].to_string()));
            }
            pieces.push(piece);
            at = range.end;
        }
        if at < src.len() {
            pieces.push(Piece::Text(src[at..].to_string()));
        }
        Self(pieces)
    }

    pub fn has_number(&self) -> bool {
        self.0.contains(&Piece::Number)
    }
}

/// Each token in its canonical spelling, so `Template::parse(&t.to_string()) == t`.
impl fmt::Display for Template {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for piece in &self.0 {
            match piece {
                Piece::Text(t) => f.write_str(t)?,
                Piece::Name => f.write_str("[Name]")?,
                Piece::Number => f.write_str("[Number]")?,
                Piece::Group(k) => write!(f, "[Group {k}]")?,
            }
        }
        Ok(())
    }
}

/// What Find matches: the whole name when empty, else every match of its text or
/// pattern.
#[derive(Clone, Debug)]
pub struct Find(Option<Regex>);

impl Find {
    /// `text` as typed. Matching ignores case unless `match_case`; `pattern` reads
    /// `text` as a regular expression, and a pattern that does not parse is `Err`
    /// with a one-line reason.
    pub fn new(text: &str, match_case: bool, pattern: bool) -> Result<Self, String> {
        if text.is_empty() {
            return Ok(Self(None));
        }
        let src = if pattern {
            text.to_string()
        } else {
            regex::escape(text)
        };
        RegexBuilder::new(&src)
            .case_insensitive(!match_case)
            .build()
            .map(|re| Self(Some(re)))
            .map_err(|e| reason(&e))
    }

    /// Whether Find is empty, so the whole name is replaced.
    pub fn is_whole(&self) -> bool {
        self.0.is_none()
    }

    /// The pattern's bracketed groups, which the template can insert.
    pub fn groups(&self) -> usize {
        self.0.as_ref().map_or(0, |re| re.captures_len() - 1)
    }
}

/// The last line of a regex error, which names the fault without the caret drawing.
fn reason(e: &regex::Error) -> String {
    let text = e.to_string();
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    let line = line.trim().trim_start_matches("error:").trim();
    if line.is_empty() {
        "Not a pattern".to_string()
    } else {
        format!("Not a pattern: {line}")
    }
}

/// How a run is numbered: `from`, then `from + step` and on, each padded with
/// zeros to at least `pad` digits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Numbering {
    pub from: i64,
    pub step: i64,
    /// 0 to [`Numbering::MAX_PAD`]; 0 and 1 pad nothing.
    pub pad: u8,
}

impl Default for Numbering {
    fn default() -> Self {
        Self {
            from: 1,
            step: 1,
            pad: 0,
        }
    }
}

impl Numbering {
    /// The widest padding offered.
    pub const MAX_PAD: u8 = 5;

    /// The number of the run's `i`th layer, counted from 0.
    pub fn nth(&self, i: usize) -> String {
        let i = i64::try_from(i).unwrap_or(i64::MAX);
        let n = self.from.saturating_add(self.step.saturating_mul(i));
        let sign = if n < 0 { "-" } else { "" };
        format!(
            "{sign}{:0width$}",
            n.unsigned_abs(),
            width = usize::from(self.pad.min(Self::MAX_PAD))
        )
    }
}

/// A find, a template and whether a match's case carries to what replaces it.
#[derive(Clone, Debug)]
pub struct Rule {
    pub find: Find,
    pub template: Template,
    /// Only with Find filled: replacing the whole name has no match to read.
    pub keep_case: bool,
}

/// One run of a new name: kept from the old one, or new.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub changed: bool,
}

/// What [`Rule::apply`] made of one name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renamed {
    /// The byte ranges of the old name that matched.
    pub matched: Vec<Range<usize>>,
    /// The new name, before it is cleaned as a typed name is.
    pub spans: Vec<Span>,
}

impl Renamed {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

impl Rule {
    /// `name` renamed, with `number` for each Number token, or `None` when Find
    /// matches nothing in it.
    pub fn apply(&self, name: &str, number: &str) -> Option<Renamed> {
        let mut out = Out::default();
        let Some(re) = &self.find.0 else {
            for piece in &self.template.0 {
                match piece {
                    Piece::Text(t) => out.push(t, true),
                    Piece::Name => out.push(name, false),
                    Piece::Number => out.push(number, true),
                    Piece::Group(_) => {}
                }
            }
            return Some(Renamed {
                matched: std::iter::once(0..name.len()).collect(),
                spans: out.0,
            });
        };
        let mut matched = Vec::new();
        let mut at = 0;
        for caps in re.captures_iter(name) {
            let Some(m) = caps.get(0) else { continue };
            out.push(&name[at..m.start()], false);
            let mut rep = Out::default();
            for piece in &self.template.0 {
                match piece {
                    Piece::Text(t) => rep.push(t, true),
                    Piece::Name => rep.push(m.as_str(), false),
                    Piece::Number => rep.push(number, true),
                    Piece::Group(k) => rep.push(caps.get(*k).map_or("", |g| g.as_str()), false),
                }
            }
            if self.keep_case {
                rep.match_case(m.as_str());
            }
            for s in rep.0 {
                out.push(&s.text, s.changed);
            }
            matched.push(m.range());
            at = m.end();
        }
        if matched.is_empty() {
            return None;
        }
        out.push(&name[at..], false);
        Some(Renamed {
            matched,
            spans: out.0,
        })
    }
}

#[derive(Default)]
struct Out(Vec<Span>);

impl Out {
    /// Append `text`, joining it to the last span when both are kept or both new.
    fn push(&mut self, text: &str, changed: bool) {
        if text.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.changed == changed => last.text.push_str(text),
            _ => self.0.push(Span {
                text: text.to_string(),
                changed,
            }),
        }
    }

    /// Give the new spans the case `matched` is written in: `BUTTON` makes them
    /// upper case, `button` lower, `Button` a capital then lower case. A mixed
    /// case such as `iconButton` leaves them as typed.
    fn match_case(&mut self, matched: &str) {
        let letters: Vec<char> = matched.chars().filter(|c| c.is_alphabetic()).collect();
        let Some(first) = letters.first() else { return };
        let rest_lower = letters[1..].iter().all(|c| !c.is_uppercase());
        let all_upper = letters.iter().all(|c| !c.is_lowercase());
        let all_lower = letters.iter().all(|c| !c.is_uppercase());
        let new = self.0.iter_mut().filter(|s| s.changed);
        if all_upper && letters.len() > 1 {
            new.for_each(|s| s.text = s.text.to_uppercase());
        } else if all_lower {
            new.for_each(|s| s.text = s.text.to_lowercase());
        } else if first.is_uppercase() && rest_lower {
            new.for_each(|s| s.text = s.text.to_lowercase());
            // A capital only where the replacement starts with new text.
            if let Some(s) = self.0.iter_mut().find(|s| !s.text.is_empty())
                && s.changed
                && let Some(c) = s.text.chars().next()
            {
                let upper: String = c.to_uppercase().collect();
                s.text.replace_range(..c.len_utf8(), &upper);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(find: &str, template: &str) -> Rule {
        Rule {
            find: Find::new(find, false, false).unwrap(),
            template: Template::parse(template),
            keep_case: false,
        }
    }

    fn new_name(rule: &Rule, name: &str) -> Option<String> {
        rule.apply(name, "1").map(|r| r.text())
    }

    /// The typed short forms read as their tokens and print canonically; other
    /// brackets stay text.
    #[test]
    fn tokens_read_their_short_forms() {
        let t = Template::parse("Card [n] of [NAME] [x] [[name] [group 2] [3] [0]");
        assert_eq!(
            t.to_string(),
            "Card [Number] of [Name] [x] [[Name] [Group 2] [Group 3] [0]"
        );
        assert!(t.has_number());
        assert!(!Template::parse("[Name] [").has_number());
    }

    /// An empty find replaces the whole name; Name is that name.
    #[test]
    fn an_empty_find_replaces_the_whole_name() {
        let r = rule("", "[Number] · [Name]");
        let out = r.apply("Welcome", "01").unwrap();
        assert_eq!(out.text(), "01 · Welcome");
        assert_eq!(out.matched, vec![0..7]);
        assert_eq!(
            out.spans,
            vec![
                Span {
                    text: "01 · ".into(),
                    changed: true
                },
                Span {
                    text: "Welcome".into(),
                    changed: false
                },
            ]
        );
    }

    /// A filled find replaces every match, ignoring case unless asked.
    #[test]
    fn a_find_replaces_every_match() {
        assert_eq!(
            new_name(&rule("a", "o"), "Banana").as_deref(),
            Some("Bonono")
        );
        assert_eq!(
            new_name(&rule("button", "CTA"), "Button / Primary").as_deref(),
            Some("CTA / Primary")
        );
        assert_eq!(new_name(&rule("x", "y"), "Card"), None);
        let exact = Rule {
            find: Find::new("Button", true, false).unwrap(),
            template: Template::parse("CTA"),
            keep_case: false,
        };
        assert_eq!(new_name(&exact, "Icon button"), None);
        assert_eq!(
            new_name(&exact, "Button label").as_deref(),
            Some("CTA label")
        );
    }

    /// A literal find is not a pattern: its metacharacters are text.
    #[test]
    fn a_literal_find_escapes_its_text() {
        assert_eq!(
            new_name(&rule("(1)", "#"), "Card (1)").as_deref(),
            Some("Card #")
        );
        assert_eq!(new_name(&rule(".", "-"), "a.b").as_deref(), Some("a-b"));
    }

    /// Keep case writes the replacement in each match's case.
    #[test]
    fn keep_case_follows_each_match() {
        let r = Rule {
            keep_case: true,
            ..rule("button", "cta")
        };
        assert_eq!(new_name(&r, "buttonGhost").as_deref(), Some("ctaGhost"));
        assert_eq!(
            new_name(&r, "Button / Large").as_deref(),
            Some("Cta / Large")
        );
        assert_eq!(new_name(&r, "BUTTON_LABEL").as_deref(), Some("CTA_LABEL"));
        assert_eq!(new_name(&r, "bUtToN").as_deref(), Some("cta"));
        let upper = Rule {
            keep_case: true,
            ..rule("button", "CTA")
        };
        assert_eq!(new_name(&upper, "Button").as_deref(), Some("Cta"));
    }

    /// A pattern's groups reach the template; an empty template deletes the match.
    #[test]
    fn a_pattern_reads_groups() {
        let p = |template: &str| Rule {
            find: Find::new(r"^(\d+) · ", false, true).unwrap(),
            template: Template::parse(template),
            keep_case: false,
        };
        assert_eq!(p("").find.groups(), 1);
        assert_eq!(new_name(&p(""), "01 · Welcome").as_deref(), Some("Welcome"));
        assert_eq!(
            new_name(&p("Step [Group 1]: "), "01 · Welcome").as_deref(),
            Some("Step 01: Welcome")
        );
        assert_eq!(
            new_name(&p("[Group 2]"), "01 · Welcome").as_deref(),
            Some("Welcome")
        );
    }

    /// A pattern that does not parse says why in one line.
    #[test]
    fn a_bad_pattern_says_why() {
        let e = Find::new("(abc", false, true).unwrap_err();
        assert!(e.starts_with("Not a pattern: "), "{e}");
        assert!(!e.contains('\n'), "{e}");
        assert!(Find::new("(abc", false, false).is_ok());
    }

    /// Numbers pad to the digits asked for, never past the widest, and a negative
    /// step counts down through zero.
    #[test]
    fn numbering_pads_and_counts_down() {
        let n = Numbering {
            from: 1,
            step: 1,
            pad: 2,
        };
        assert_eq!(
            (0..3).map(|i| n.nth(i)).collect::<Vec<_>>(),
            ["01", "02", "03"]
        );
        let down = Numbering {
            from: 1,
            step: -1,
            pad: 3,
        };
        assert_eq!(
            (0..3).map(|i| down.nth(i)).collect::<Vec<_>>(),
            ["001", "000", "-001"]
        );
        assert_eq!(Numbering::default().nth(9), "10");
        let wide = Numbering {
            pad: 9,
            ..Numbering::default()
        };
        assert_eq!(wide.nth(0), "00001", "capped at five");
    }
}
