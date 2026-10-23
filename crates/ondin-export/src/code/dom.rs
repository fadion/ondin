//! The tree the walk builds and the emitters print: elements, their class and
//! their declarations, before any target's syntax.

use std::collections::{HashMap, HashSet};
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
    /// In a set's merged tree, drawn only under these variants (`Show::Under`)
    /// or under all but these (`Show::Except`).
    pub shown_when: Option<Show>,
    /// The layer's step in its name path, which matches it across a set's
    /// variant trees; `None` for an element drawn for no layer of its own.
    pub key: Option<(String, usize)>,
    /// A component's root, which takes the class its user places it with.
    pub takes_class: bool,
    /// The Vue directive choosing this variant's tree (`v-if="…"`, `v-else`),
    /// written whole.
    pub branch: Option<String>,
    /// In a set's merged tree, the classes worn instead of [`Self::class`] under
    /// other variants, the first whose values hold winning. Each rule is whole,
    /// so exactly one of the element's rules applies.
    pub alts: Vec<Alt>,
}

/// A class an element wears under some of its set's variants.
#[derive(Debug, Clone)]
pub(crate) struct Alt {
    /// The variant values it is worn for, any one of them.
    pub when: Vec<When>,
    pub class: String,
    pub style: Vec<(&'static str, String)>,
}

/// Variant values a tree or class is drawn for, as `(prop, value)`: every one
/// must hold.
pub(crate) type When = Vec<(String, String)>;

/// Which variants a merged tree's element is drawn under.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Show {
    /// Under any of these.
    Under(Vec<When>),
    /// Under any but these: the default's tree draws it, and that tree is drawn
    /// for any values no other tree is.
    Except(Vec<When>),
}

/// `when` as a condition: JSX's double-quoted strings, Vue's single-quoted ones.
pub(crate) fn condition(when: &[(String, String)], syntax: Syntax) -> String {
    let terms: Vec<String> = when
        .iter()
        .map(|(p, v)| match syntax {
            Syntax::Jsx => format!("{p} === {}", super::jsx::js_string(v)),
            _ => format!("{p} === {}", super::jsx::single_quoted(v)),
        })
        .collect();
    terms.join(" && ")
}

/// A condition holding when any of `whens` does.
fn any_of(whens: &[When], syntax: Syntax) -> String {
    let parts: Vec<String> = whens
        .iter()
        .map(|w| match (whens.len(), w.len()) {
            (1, _) | (_, 1) => condition(w, syntax),
            _ => format!("({})", condition(w, syntax)),
        })
        .collect();
    parts.join(" || ")
}

/// A use of a component: its name and the properties it sets.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Instance {
    pub component: String,
    pub args: Vec<(String, Arg)>,
    /// The swap property choosing the component, when the use is its slot:
    /// [`Self::component`] is then only the default.
    pub slot: Option<String>,
}

/// A component property's value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Arg {
    Bool(bool),
    Text(String),
    /// A component, by name: a swap property's.
    Component(String),
    /// The user's own prop `name`, passed on; `fallback` where the prop is
    /// left unset. A `component` prop is read through its JSX binding.
    Var {
        name: String,
        component: bool,
        fallback: Option<Box<Arg>>,
    },
}

/// What an element holds.
#[derive(Debug, Clone)]
pub(crate) enum Child {
    Element(Box<Element>),
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
    /// A Vue template: HTML, with text Vue's compiler would change written as an
    /// interpolated string ([`vue_text`]).
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

    /// The condition the element is drawn under — its boolean property's and
    /// its variants' — or `None` when it is always drawn.
    fn guard(&self, syntax: Syntax) -> Option<String> {
        let when = self.shown_when.as_ref().map(|s| match s {
            Show::Under(w) if w.len() == 1 => condition(&w[0], syntax),
            Show::Under(w) => format!("({})", any_of(w, syntax)),
            Show::Except(w) => format!("!({})", any_of(w, syntax)),
        });
        match (&self.shown_by, when) {
            (Some(p), Some(w)) => Some(format!("{p} && {w}")),
            (Some(p), None) => Some(p.clone()),
            (None, w) => w,
        }
    }

    /// The element and everything under it in `syntax`, indented by `depth`.
    pub fn markup(&self, out: &mut String, depth: usize, syntax: Syntax) {
        // JSX shows a guarded element through an expression around it; Vue puts
        // a `v-if` on the element itself.
        if let (Some(guard), Syntax::Jsx) = (self.guard(syntax), syntax) {
            let pad = "  ".repeat(depth);
            let _ = writeln!(out, "{pad}{{{guard} && (");
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
                Syntax::Vue => super::jsx::vue_svg(m),
            };
            if let (Some(guard), Syntax::Vue) = (self.guard(syntax), syntax) {
                m = m.replacen("<svg ", &format!("<svg v-if=\"{}\" ", escape(&guard)), 1);
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
    /// A slot's component is the swap property's value: a binding in JSX, a
    /// dynamic `component` in Vue.
    fn use_of(&self, out: &mut String, i: &Instance, syntax: Syntax) {
        match (&i.slot, syntax) {
            (Some(prop), Syntax::Jsx) => {
                let _ = write!(out, "<{}", super::jsx::slot_binding(prop));
            }
            (Some(prop), _) => {
                let _ = write!(out, "<component :is=\"{prop}\"");
            }
            (None, _) => {
                let _ = write!(out, "<{}", i.component);
            }
        }
        self.class_attr(out, syntax);
        if let (Some(guard), Syntax::Vue) = (self.guard(syntax), syntax) {
            let _ = write!(out, " v-if=\"{}\"", escape(&guard));
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
                (Arg::Component(c), Syntax::Jsx) => {
                    let _ = write!(out, " {name}={{{c}}}");
                }
                (Arg::Component(c), _) => {
                    let _ = write!(out, " :{name}=\"{c}\"");
                }
                (Arg::Var { .. }, Syntax::Jsx) => {
                    let _ = write!(out, " {name}={{{}}}", arg_expr(arg, syntax));
                }
                (Arg::Var { .. }, _) => {
                    let _ = write!(out, " :{name}=\"{}\"", escape(&arg_expr(arg, syntax)));
                }
            }
        }
        out.push_str(" />");
    }

    fn class_attr(&self, out: &mut String, syntax: Syntax) {
        if self.class.is_empty() {
            return;
        }
        // The class as an expression: a module's binding in JSX, a string in
        // Vue, each alternative behind its condition.
        let one = |class: &str| match syntax {
            Syntax::Jsx => {
                let m = super::jsx::module_class(class);
                m[1..m.len() - 1].to_string()
            }
            _ => format!("'{class}'"),
        };
        let mut expr = String::new();
        for a in &self.alts {
            let _ = write!(expr, "{} ? {} : ", any_of(&a.when, syntax), one(&a.class));
        }
        expr.push_str(&one(&self.class));
        match syntax {
            // The class a component root is placed with joins its own.
            Syntax::Jsx if self.takes_class => {
                let _ = write!(out, " className={{`${{{expr}}} ${{className ?? \"\"}}`}}");
            }
            Syntax::Jsx if !self.alts.is_empty() => {
                let _ = write!(out, " className={{{expr}}}");
            }
            Syntax::Jsx => {
                let _ = write!(out, " className={}", super::jsx::module_class(&self.class));
            }
            Syntax::Vue if !self.alts.is_empty() => {
                let _ = write!(out, " :class=\"{}\"", escape(&expr));
            }
            _ => {
                let _ = write!(out, " class=\"{}\"", escape(&self.class));
            }
        }
    }

    /// [`Self::classes`], to rename.
    pub fn classes_mut(&mut self) -> impl Iterator<Item = &mut String> {
        std::iter::once(&mut self.class).chain(self.alts.iter_mut().map(|a| &mut a.class))
    }

    /// Every class the element may wear: its own, then its alternatives'.
    pub fn classes(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.class).chain(self.alts.iter().map(|a| &a.class))
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
            if let Some(b) = &self.branch {
                let _ = write!(out, " {b}");
            }
            if let Some(guard) = self.guard(syntax) {
                let _ = write!(out, " v-if=\"{}\"", escape(&guard));
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
        let alts = self.alts.iter().map(|a| (&a.class, &a.style));
        for (class, style) in std::iter::once((&self.class, &self.style)).chain(alts) {
            if class.is_empty() || style.is_empty() {
                continue;
            }
            if self.instance.is_some() {
                let _ = writeln!(out, ".{class}.{class} {{");
            } else {
                let _ = writeln!(out, ".{class} {{");
            }
            for (p, v) in style {
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
    /// Each base's last suffix: every one below it is taken, and nothing is
    /// ever freed, so the search resumes there rather than at 1 — a page of
    /// thousands of *Rectangle*s would otherwise mint in quadratic time.
    next: HashMap<String, usize>,
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
        let mut n = self.next.get(&base).copied().unwrap_or(1);
        let mut candidate = match n {
            1 => base.clone(),
            n => format!("{base}-{n}"),
        };
        while self.taken.contains(&candidate) {
            n += 1;
            candidate = format!("{base}-{n}");
        }
        self.next.insert(base, n);
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
        Syntax::Vue => vue_text(t),
        Syntax::Html => escape(t),
    }
}

/// Text in a Vue template: as written where Vue's compiler leaves it alone, else
/// an interpolated string. The compiler condenses runs of whitespace and trims
/// it at the edges, and reads `{{` as an interpolation; a `v-pre` would also
/// void the directives on its element.
pub(crate) fn vue_text(t: &str) -> String {
    let plain = !t.is_empty()
        && t.trim() == t
        && !t.contains(['\n', '\r', '\t'])
        && !t.contains("  ")
        && !t.contains("{{");
    if plain {
        return escape(t);
    }
    // No brace in the string, which `}}` would end early.
    let literal = super::jsx::single_quoted(t)
        .replace('{', "\\u007b")
        .replace('}', "\\u007d");
    format!("{{{{ {} }}}}", escape(&literal))
}

/// `arg` as an expression: JSX's double-quoted strings, Vue's single-quoted ones
/// (the template puts the expression in a double-quoted attribute).
fn arg_expr(arg: &Arg, syntax: Syntax) -> String {
    match arg {
        Arg::Bool(b) => b.to_string(),
        Arg::Text(t) => match syntax {
            Syntax::Jsx => super::jsx::js_string(t),
            _ => super::jsx::single_quoted(t),
        },
        Arg::Component(c) => c.clone(),
        Arg::Var {
            name,
            component,
            fallback,
        } => {
            let name = match (component, syntax) {
                (true, Syntax::Jsx) => super::jsx::slot_binding(name),
                _ => name.clone(),
            };
            match fallback {
                Some(f) => format!("{name} ?? {}", arg_expr(f, syntax)),
                None => name,
            }
        }
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

    /// Minting resumes at a base's last suffix and names as the search from 1
    /// would, a literal `card-3` still skipped.
    ///
    /// flip: `Classes::mint` searching from 1 each time → the counter assertion.
    #[test]
    fn a_repeated_name_mints_from_its_last_suffix() {
        let mut c = Classes::default();
        assert_eq!(c.mint("Card 3", "Frame"), "card-3");
        let names: Vec<String> = (0..4).map(|_| c.mint("Card", "Frame")).collect();
        assert_eq!(names, ["card", "card-2", "card-4", "card-5"]);
        assert_eq!(c.next.get("card"), Some(&5));
    }

    /// Text Vue's compiler would condense, trim or interpolate is a string it
    /// leaves alone, braces and markup escaped; plain text is as written.
    ///
    /// flip: `vue_text` passing `\t` through → fails at `a\tb`, which Vue would
    /// condense to a space.
    #[test]
    fn vue_text_survives_the_compiler() {
        assert_eq!(vue_text("Sign in"), "Sign in");
        assert_eq!(vue_text("Total:   $5\tdue"), "{{ 'Total:   $5\\tdue' }}");
        assert_eq!(vue_text("a\tb"), "{{ 'a\\tb' }}");
        assert_eq!(vue_text(" lead"), "{{ ' lead' }}");
        assert_eq!(
            vue_text("a {{ b }}"),
            "{{ 'a \\u007b\\u007b b \\u007d\\u007d' }}"
        );
        assert_eq!(vue_text("<i>  \"q\""), "{{ '&lt;i&gt;  &quot;q&quot;' }}");
    }

    /// No element carries `v-pre`, which would void a `v-if` beside it.
    ///
    /// flip: `Element::open` writing `v-pre` on text holding `{{` → fails at "v-pre".
    #[test]
    fn a_guarded_text_with_braces_keeps_its_guard_in_vue() {
        let mut p = Element::new("p", "note".into());
        p.shown_by = Some("showNote".into());
        p.children.push(Child::Text("Hi {{ name }}".into()));
        let mut out = String::new();
        p.markup(&mut out, 0, Syntax::Vue);
        assert!(!out.contains("v-pre"), "{out}");
        assert!(out.contains("v-if=\"showNote\""), "{out}");
    }
}
