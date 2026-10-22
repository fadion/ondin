//! The tree the walk builds and the emitters print: elements, their class and
//! their declarations, before any target's syntax.

use std::collections::HashSet;
use std::fmt::Write as _;

/// One DOM element. Its declarations become one rule on its class, which is
/// unique in the export.
#[derive(Debug, Clone, Default)]
pub(crate) struct Element {
    pub tag: &'static str,
    pub class: String,
    pub attrs: Vec<(&'static str, String)>,
    pub style: Vec<(&'static str, String)>,
    pub children: Vec<Child>,
    /// The whole element as markup another writer made (an inline `<svg>` already
    /// carrying [`Self::class`]), printed in place of a tag and children.
    pub markup: Option<String>,
}

/// What an element holds.
#[derive(Debug, Clone)]
pub(crate) enum Child {
    Element(Element),
    /// Character data, escaped when printed.
    Text(String),
    /// Markup already written by another writer (an inline `<svg>`), printed as is.
    Markup(String),
}

impl Element {
    pub fn new(tag: &'static str, class: String) -> Self {
        Element {
            tag,
            class,
            ..Default::default()
        }
    }

    /// Add a declaration. A later one for the same property replaces the earlier,
    /// so the rule never states a property twice.
    pub fn set(&mut self, prop: &'static str, value: impl Into<String>) {
        let value = value.into();
        match self.style.iter_mut().find(|(p, _)| *p == prop) {
            Some(slot) => slot.1 = value,
            None => self.style.push((prop, value)),
        }
    }

    /// Whether whitespace between children would change what is drawn: text
    /// elements are printed on one line.
    fn holds_text(&self) -> bool {
        self.children.iter().any(|c| matches!(c, Child::Text(_)))
            || self.tag == "p"
            || self.tag == "span"
    }

    /// The element and everything under it as HTML, indented by `depth`.
    pub fn html(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        if let Some(m) = &self.markup {
            for line in m.lines() {
                let _ = writeln!(out, "{pad}{line}");
            }
            return;
        }
        out.push_str(&pad);
        self.open(out);
        if self.tag == "img" {
            out.push('\n');
            return;
        }
        if self.holds_text() {
            self.inline_children(out);
            let _ = writeln!(out, "</{}>", self.tag);
            return;
        }
        out.push('\n');
        for c in &self.children {
            match c {
                Child::Element(e) => e.html(out, depth + 1),
                Child::Text(t) => {
                    let _ = writeln!(out, "{pad}  {}", escape(t));
                }
                Child::Markup(m) => {
                    for line in m.lines() {
                        let _ = writeln!(out, "{pad}  {line}");
                    }
                }
            }
        }
        let _ = writeln!(out, "{pad}</{}>", self.tag);
    }

    fn open(&self, out: &mut String) {
        let _ = write!(out, "<{}", self.tag);
        if !self.class.is_empty() {
            let _ = write!(out, " class=\"{}\"", escape(&self.class));
        }
        for (k, v) in &self.attrs {
            let _ = write!(out, " {k}=\"{}\"", escape(v));
        }
        out.push('>');
    }

    fn inline_children(&self, out: &mut String) {
        for c in &self.children {
            match c {
                Child::Element(e) => {
                    e.open(out);
                    e.inline_children(out);
                    let _ = write!(out, "</{}>", e.tag);
                }
                Child::Text(t) => out.push_str(&escape(t)),
                Child::Markup(m) => out.push_str(m),
            }
        }
    }

    /// Every rule in the subtree, in document order.
    pub fn css(&self, out: &mut String) {
        if !self.class.is_empty() && !self.style.is_empty() {
            let _ = writeln!(out, ".{} {{", self.class);
            for (p, v) in &self.style {
                let _ = writeln!(out, "  {p}: {v};");
            }
            out.push_str("}\n");
        }
        for c in &self.children {
            if let Child::Element(e) = c {
                e.css(out);
            }
        }
    }
}

/// Class names from layer names: lowercase ASCII words joined by `-`, the kind's
/// default name standing in for a name with nothing usable, and a numeric suffix
/// on the second and later uses of one name.
#[derive(Default)]
pub(crate) struct Classes {
    taken: HashSet<String>,
}

impl Classes {
    pub fn mint(&mut self, name: &str, fallback: &str) -> String {
        let mut base = slug(name);
        if base.is_empty() {
            base = slug(fallback);
        }
        if base.is_empty() {
            base = "layer".to_string();
        }
        // A class may not start with a digit.
        if base.starts_with(|c: char| c.is_ascii_digit()) {
            base.insert_str(0, "l-");
        }
        // `card-2` may itself be a layer's name, so a suffix is checked like any name.
        let mut candidate = base.clone();
        let mut n = 1;
        while self.taken.contains(&candidate) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        self.taken.insert(candidate.clone());
        candidate
    }
}

/// `"Primary Button / Hover"` -> `primary-button-hover`.
pub(crate) fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    out
}

/// Escape for HTML text and double-quoted attributes alike.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names slug to lowercase words; repeats and digit-led names stay valid classes.
    #[test]
    fn class_names_are_slugged_and_unique() {
        let mut c = Classes::default();
        assert_eq!(
            c.mint("Primary Button / Hover", "Rectangle"),
            "primary-button-hover"
        );
        assert_eq!(c.mint("Card", "Frame"), "card");
        assert_eq!(c.mint("card", "Frame"), "card-2");
        assert_eq!(c.mint("Card 2", "Frame"), "card-2-2");
        assert_eq!(c.mint("✦", "Ellipse"), "ellipse");
        assert_eq!(c.mint("404", "Text"), "l-404");
    }
}
