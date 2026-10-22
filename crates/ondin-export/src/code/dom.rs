//! The tree the walk builds and the emitters print: elements, their class and
//! their declarations, before any target's syntax.

use std::collections::HashSet;
use std::fmt::Write as _;

/// One DOM element. Its declarations become one rule on its class, which is
/// unique in its file.
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
    /// Written as a use of a component, whose root this element's declarations
    /// place; its rule doubles the class to win over the component's own.
    pub instance: Option<Instance>,
    /// Drawn only while this boolean property is true.
    pub shown_by: Option<String>,
    /// A component's root, which takes the class its user places it with.
    pub takes_class: bool,
}

/// A use of a component: its name and the properties it sets.
#[derive(Debug, Clone, Default)]
pub(crate) struct Instance {
    pub component: String,
    pub args: Vec<(String, Arg)>,
}

/// A component property's value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Arg {
    Bool(bool),
    Text(String),
}

/// What an element holds.
#[derive(Debug, Clone)]
pub(crate) enum Child {
    Element(Element),
    /// Character data, escaped when printed.
    Text(String),
    /// A line break, which an empty paragraph holds to take its line.
    Break,
    /// A text property's value.
    Prop(String),
}

/// The markup an element tree is printed as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Syntax {
    Html,
    /// JSX against a CSS module imported as `styles`, an `img`'s `src` naming the
    /// binding its file is imported as ([`super::jsx::asset_binding`]).
    Jsx,
    /// A Vue template: HTML, with `v-pre` where text holds `{{`, which Vue would
    /// read as an interpolation.
    Vue,
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

    /// The element and everything under it in `syntax`, indented by `depth`.
    pub fn markup(&self, out: &mut String, depth: usize, syntax: Syntax) {
        // JSX shows a bound element through an expression around it; Vue puts a
        // `v-if` on the element itself.
        if let (Some(prop), Syntax::Jsx) = (&self.shown_by, syntax) {
            let pad = "  ".repeat(depth);
            let _ = writeln!(out, "{pad}{{{prop} && (");
            self.markup_shown(out, depth + 1, syntax);
            let _ = writeln!(out, "{pad})}}");
            return;
        }
        self.markup_shown(out, depth, syntax);
    }

    fn markup_shown(&self, out: &mut String, depth: usize, syntax: Syntax) {
        let pad = "  ".repeat(depth);
        if let Some(m) = &self.markup {
            let mut m = match syntax {
                Syntax::Html => m.clone(),
                Syntax::Jsx => super::jsx::from_svg(m),
                Syntax::Vue if m.contains("{{") => m.replacen("<svg ", "<svg v-pre ", 1),
                Syntax::Vue => m.clone(),
            };
            if let (Some(prop), Syntax::Vue) = (&self.shown_by, syntax) {
                m = m.replacen("<svg ", &format!("<svg v-if=\"{prop}\" "), 1);
            }
            for line in m.lines() {
                let _ = writeln!(out, "{pad}{line}");
            }
            return;
        }
        out.push_str(&pad);
        if let Some(i) = &self.instance {
            self.use_of(out, i, syntax);
            out.push('\n');
            return;
        }
        if self.tag == "img" {
            self.open(out, syntax, true);
            out.push('\n');
            return;
        }
        if self.holds_text() {
            self.open(out, syntax, false);
            self.inline_children(out, syntax);
            let _ = writeln!(out, "</{}>", self.tag);
            return;
        }
        self.open(out, syntax, false);
        out.push('\n');
        for c in &self.children {
            match c {
                Child::Element(e) => e.markup(out, depth + 1, syntax),
                Child::Text(t) => {
                    let _ = writeln!(out, "{pad}  {}", text(t, syntax));
                }
                Child::Break => {
                    let _ = writeln!(out, "{pad}  {}", br(syntax));
                }
                Child::Prop(p) => {
                    let _ = writeln!(out, "{pad}  {}", prop(p, syntax));
                }
            }
        }
        let _ = writeln!(out, "{pad}</{}>", self.tag);
    }

    /// A component's tag, placed by this element's class, with its arguments.
    fn use_of(&self, out: &mut String, i: &Instance, syntax: Syntax) {
        let _ = write!(out, "<{}", i.component);
        self.class_attr(out, syntax);
        if let (Some(prop), Syntax::Vue) = (&self.shown_by, syntax) {
            let _ = write!(out, " v-if=\"{prop}\"");
        }
        for (name, arg) in &i.args {
            match (arg, syntax) {
                (Arg::Bool(b), Syntax::Jsx) => {
                    let _ = write!(out, " {name}={{{b}}}");
                }
                (Arg::Bool(b), _) => {
                    let _ = write!(out, " :{name}=\"{b}\"");
                }
                (Arg::Text(t), Syntax::Jsx)
                    if !t.contains('"') && super::jsx::child_text(t) == *t =>
                {
                    let _ = write!(out, " {name}=\"{t}\"");
                }
                (Arg::Text(t), Syntax::Jsx) => {
                    let _ = write!(out, " {name}={{{}}}", super::jsx::js_string(t));
                }
                (Arg::Text(t), _) => {
                    let _ = write!(out, " {name}=\"{}\"", escape(t));
                }
            }
        }
        out.push_str(" />");
    }

    fn class_attr(&self, out: &mut String, syntax: Syntax) {
        if self.class.is_empty() {
            return;
        }
        match syntax {
            // The class a component root is placed with joins its own.
            Syntax::Jsx if self.takes_class => {
                let own = super::jsx::module_class(&self.class);
                let _ = write!(
                    out,
                    " className={{`${{{}}} ${{className ?? \"\"}}`}}",
                    &own[1..own.len() - 1]
                );
            }
            Syntax::Jsx => {
                let _ = write!(out, " className={}", super::jsx::module_class(&self.class));
            }
            _ => {
                let _ = write!(out, " class=\"{}\"", escape(&self.class));
            }
        }
    }

    /// The opening tag; `void` closes it as an element with no content.
    fn open(&self, out: &mut String, syntax: Syntax, void: bool) {
        let _ = write!(out, "<{}", self.tag);
        self.class_attr(out, syntax);
        for (k, v) in &self.attrs {
            if syntax == Syntax::Jsx && *k == "src" {
                let _ = write!(out, " src={{{}}}", super::jsx::asset_binding(v));
            } else {
                let _ = write!(out, " {k}=\"{}\"", escape(v));
            }
        }
        if syntax == Syntax::Vue {
            if let Some(prop) = &self.shown_by {
                let _ = write!(out, " v-if=\"{prop}\"");
            }
            if self.holds_text() && self.text_has("{{") {
                out.push_str(" v-pre");
            }
        }
        out.push_str(match (void, syntax) {
            (true, Syntax::Jsx) => " />",
            _ => ">",
        });
    }

    fn inline_children(&self, out: &mut String, syntax: Syntax) {
        for c in &self.children {
            match c {
                Child::Element(e) => {
                    e.open(out, syntax, false);
                    e.inline_children(out, syntax);
                    let _ = write!(out, "</{}>", e.tag);
                }
                Child::Text(t) => out.push_str(&text(t, syntax)),
                Child::Break => out.push_str(br(syntax)),
                Child::Prop(p) => out.push_str(&prop(p, syntax)),
            }
        }
    }

    /// Whether any text in the subtree contains `needle`.
    fn text_has(&self, needle: &str) -> bool {
        self.children.iter().any(|c| match c {
            Child::Element(e) => e.text_has(needle),
            Child::Text(t) => t.contains(needle),
            Child::Break | Child::Prop(_) => false,
        })
    }

    /// Every element in the subtree, this one first, in document order.
    pub fn each(&self, f: &mut impl FnMut(&Element)) {
        f(self);
        for c in &self.children {
            if let Child::Element(e) = c {
                e.each(f);
            }
        }
    }

    /// Every rule in the subtree, in document order. A component use's rule names
    /// its class twice, so it outranks the component root's own rule whichever
    /// stylesheet loads last.
    pub fn css(&self, out: &mut String) {
        if !self.class.is_empty() && !self.style.is_empty() {
            if self.instance.is_some() {
                let _ = writeln!(out, ".{0}.{0} {{", self.class);
            } else {
                let _ = writeln!(out, ".{} {{", self.class);
            }
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

/// Character data in `syntax`.
fn text(t: &str, syntax: Syntax) -> String {
    match syntax {
        Syntax::Jsx => super::jsx::child_text(t),
        _ => escape(t),
    }
}

/// A text property's value as the template reads it.
fn prop(name: &str, syntax: Syntax) -> String {
    match syntax {
        Syntax::Jsx => format!("{{{name}}}"),
        _ => format!("{{{{ {name} }}}}"),
    }
}

fn br(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Jsx => "<br />",
        _ => "<br>",
    }
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
