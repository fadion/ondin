//! The tree the walk builds and the emitters print: elements, their class and
//! their declarations, before any target's syntax.

use ondin_core::variant::{Interaction, RootElement};
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
    /// variant trees once they are paired (`merge::pair`); `None` for an element
    /// drawn for no layer of its own.
    pub key: Option<(String, usize)>,
    /// The layer's lineage key, and whether it was recorded rather than its own
    /// id standing in (`Node::lineage`).
    pub lineage: Option<(ondin_core::NodeId, bool)>,
    /// What each interaction state changes on this element, in a merged tree.
    pub states: Vec<StateStyle>,
    /// A marked set's root: written with the props that drive its states.
    pub live: Option<Live>,
    /// A component's root, which takes the class its user places it with.
    pub takes_class: bool,
    /// The Vue directive choosing this variant's tree (`v-if="…"`, `v-else`),
    /// written whole.
    pub branch: Option<String>,
    /// In a set's merged tree, the classes worn instead of [`Self::class`] under
    /// other variants, the first whose values hold winning. Each rule is whole,
    /// so exactly one of the element's rules applies.
    pub alts: Vec<Alt>,
    /// The text style it wears as a class of `text-styles.css`, before its own.
    /// [`Self::style`] stays whole, so merges and states compare what is drawn;
    /// only the printed rule leaves out what the style's class declares.
    pub text_class: Option<std::rc::Rc<TextClass>>,
}

/// A text style written as a global class, which every text using it wears.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextClass {
    pub class: String,
    pub style: Vec<(&'static str, String)>,
}

impl TextClass {
    /// `style` less the declarations this class already makes. A property the
    /// class declares and `style` does not is set back to `normal`, the value
    /// such a property takes when the walk leaves it out.
    pub fn own(&self, style: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
        let mut out: Vec<_> = style
            .iter()
            .filter(|d| !self.style.contains(d))
            .cloned()
            .collect();
        for (p, _) in &self.style {
            if !style.iter().any(|(q, _)| q == p) {
                out.push((p, "normal".into()));
            }
        }
        out
    }

    /// The class's rule.
    pub fn css(&self, out: &mut String) {
        let _ = writeln!(out, ".{} {{", self.class);
        for (p, v) in &self.style {
            let _ = writeln!(out, "  {p}: {v};");
        }
        out.push_str("}\n");
    }
}

/// A class an element wears under some of its set's variants.
#[derive(Debug, Clone)]
pub(crate) struct Alt {
    /// The variant values it is worn for, any one of them.
    pub when: Vec<When>,
    pub class: String,
    pub style: Vec<(&'static str, String)>,
}

/// The declarations one interaction state writes on an element, under one of
/// its variants: the rule names the element's class and its root's, each by its
/// place in [`Element::classes`] (0 its own, then its alternatives'), so a
/// renamed class renames the rule.
#[derive(Debug, Clone)]
pub(crate) struct StateStyle {
    pub state: Interaction,
    /// The prop value that forces it, written as `data-state`.
    pub value: String,
    pub root_class: usize,
    pub class: usize,
    pub style: Vec<(&'static str, String)>,
}

/// What a marked set's root is written with: the prop forcing a state, its
/// rest value, the `disabled` prop, the value tied to Disabled, the element and
/// a link's address, and the name the props the component does not name are
/// gathered under.
#[derive(Debug, Clone)]
pub(crate) struct Live {
    pub prop: String,
    pub rest: String,
    pub disabled: String,
    pub disabled_value: Option<String>,
    pub element: RootElement,
    pub href: String,
    pub spread: String,
}

impl Live {
    fn quote(v: &str, syntax: Syntax) -> String {
        match syntax {
            Syntax::Jsx => super::jsx::js_string(v),
            _ => super::jsx::single_quoted(v),
        }
    }

    /// `disabled`, or forced Disabled, as an expression.
    fn disabled_expr(&self, syntax: Syntax) -> String {
        match &self.disabled_value {
            Some(v) => format!(
                "{} || {} === {}",
                self.disabled,
                self.prop,
                Self::quote(v, syntax)
            ),
            None => self.disabled.clone(),
        }
    }

    /// The forced value as an expression: none at rest, which leaves the
    /// states to the browser as an unset prop does.
    fn state_expr(&self, syntax: Syntax) -> String {
        format!(
            "{p} === {} ? undefined : {p}",
            Self::quote(&self.rest, syntax),
            p = self.prop
        )
    }

    /// The root's state attributes: the forced value, and the behaviour of
    /// `disabled` — the attribute on a button, `aria-disabled` on anything else.
    /// Plain HTML has no props: [`Self::drawn_attrs`] writes it for one use.
    fn attrs(&self, out: &mut String, syntax: Syntax) {
        let expr = self.disabled_expr(syntax);
        let state = self.state_expr(syntax);
        let button = self.element == RootElement::Button;
        let _ = match syntax {
            Syntax::Html => Ok(()),
            Syntax::Jsx if button => {
                write!(out, " data-state={{{state}}} disabled={{{expr}}}")
            }
            Syntax::Jsx => write!(
                out,
                " data-state={{{state}}} aria-disabled={{{expr} || undefined}}"
            ),
            Syntax::Vue if button => write!(
                out,
                " :data-state=\"{}\" :disabled=\"{}\"",
                escape(&state),
                escape(&expr)
            ),
            Syntax::Vue => write!(
                out,
                " :data-state=\"{}\" :aria-disabled=\"{}\"",
                escape(&state),
                escape(&format!("{expr} || undefined"))
            ),
        };
    }

    /// The attributes one use draws its root with, as plain HTML: the forced
    /// value when `state` is not rest, and `disabled` when it holds.
    pub fn drawn_attrs(&self, state: Option<&str>, disabled: bool) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let forced = state.filter(|s| *s != self.rest);
        if let Some(s) = forced {
            out.push(("data-state", s.to_string()));
        }
        if disabled || (forced.is_some() && forced == self.disabled_value.as_deref()) {
            out.push(match self.element {
                RootElement::Button => ("disabled", String::new()),
                _ => ("aria-disabled", "true".to_string()),
            });
        }
        out
    }

    /// The selector under which `state` applies live to a root of class
    /// `root`: never while a value is forced, and hover and press never on a
    /// disabled root. Every state's selector weighs four classes, the root's
    /// class repeated to pad the lighter ones, so two states applying at once
    /// are decided by source order, the later winning.
    fn live_selector(&self, state: Interaction, root: &str) -> String {
        let off = match self.element {
            RootElement::Button => ":not(:disabled)",
            _ => ":not([aria-disabled=\"true\"])",
        };
        match (state, self.element) {
            (Interaction::Hover, _) => format!(".{root}:hover:not([data-state]){off}"),
            (Interaction::FocusVisible, _) => {
                format!(".{root}.{root}:focus-visible:not([data-state])")
            }
            (Interaction::Active, _) => format!(".{root}:active:not([data-state]){off}"),
            (Interaction::Disabled, RootElement::Button) => {
                format!(".{root}.{root}.{root}:disabled")
            }
            (Interaction::Disabled, _) => {
                format!(".{root}.{root}.{root}[aria-disabled=\"true\"]")
            }
        }
    }
}

/// `s` as a double-quoted CSS string's content.
fn css_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Variant values a tree or class is drawn for, as `(prop, value)`: every one
/// must hold. A prop written as [`state_prop`] makes it is read as
/// [`condition`] says there.
pub(crate) type When = Vec<(String, String)>;

/// The interaction state's prop as a [`When`] names it, with its rest value
/// and the values tied to no state. Its rest holds for any value but those —
/// unset, or a state the stylesheet draws from rest's tree — and any other
/// value holds only when passed.
pub(crate) fn state_prop(prop: &str, rest: &str, plain: &[String]) -> String {
    let mut out = format!("{prop}\u{0}{rest}");
    for p in plain {
        out.push('\u{0}');
        out.push_str(p);
    }
    out
}

/// The prop a [`When`]'s term names, as code names it.
pub(crate) fn term_prop(p: &str) -> &str {
    p.split('\u{0}').next().unwrap_or(p)
}

/// Whether the term `(p, v)` holds when its prop is passed `passed`, as
/// [`condition`] writes it.
pub(crate) fn term_holds(p: &str, v: &str, passed: Option<&str>) -> bool {
    let mut parts = p.split('\u{0}').skip(1);
    match parts.next() {
        Some(rest) if rest == v => passed.is_none_or(|x| !parts.any(|plain| plain == x)),
        _ => passed == Some(v),
    }
}

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
    let quote = |v: &str| match syntax {
        Syntax::Jsx => super::jsx::js_string(v),
        _ => super::jsx::single_quoted(v),
    };
    let terms: Vec<String> = when
        .iter()
        .filter_map(|(p, v)| {
            let mut parts = p.split('\u{0}');
            let name = parts.next().unwrap_or_default();
            let Some(rest) = parts.next() else {
                return Some(format!("{p} === {}", quote(v)));
            };
            let plain: Vec<&str> = parts.collect();
            match (v == rest, plain.as_slice()) {
                (false, _) => Some(format!("{name} === {}", quote(v))),
                (true, []) => None,
                (true, [one]) => Some(format!("{name} !== {}", quote(one))),
                (true, many) => {
                    let any: Vec<String> = many
                        .iter()
                        .map(|x| format!("{name} === {}", quote(x)))
                        .collect();
                    Some(format!("!({})", any.join(" || ")))
                }
            }
        })
        .collect();
    match terms.is_empty() {
        true => "true".to_string(),
        false => terms.join(" && "),
    }
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
        // A text style's class is global, a plain string before the own class.
        let global = match &self.text_class {
            Some(t) => format!("{} ", t.class),
            None => String::new(),
        };
        match syntax {
            // The class a component root is placed with joins its own.
            Syntax::Jsx if self.takes_class => {
                let _ = write!(
                    out,
                    " className={{`{global}${{{expr}}} ${{className ?? \"\"}}`}}"
                );
            }
            Syntax::Jsx if !self.alts.is_empty() && !global.is_empty() => {
                let _ = write!(out, " className={{`{global}${{{expr}}}`}}");
            }
            Syntax::Jsx if !self.alts.is_empty() => {
                let _ = write!(out, " className={{{expr}}}");
            }
            Syntax::Jsx if !global.is_empty() => {
                let _ = write!(out, " className={{`{global}${{{expr}}}`}}");
            }
            Syntax::Jsx => {
                let _ = write!(out, " className={}", super::jsx::module_class(&self.class));
            }
            // Vue joins a static `class` with a bound one.
            Syntax::Vue if !self.alts.is_empty() => {
                if let Some(t) = &self.text_class {
                    let _ = write!(out, " class=\"{}\"", escape(&t.class));
                }
                let _ = write!(out, " :class=\"{}\"", escape(&expr));
            }
            _ => {
                let _ = write!(out, " class=\"{}{}\"", escape(&global), escape(&self.class));
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
        // What the component does not name reaches its root — `onClick`,
        // `href` — before its own attributes, which win. A link's own `href`
        // is a default, written before, so a passed one wins as Vue's
        // fallthrough lets it.
        let default_first = |k: &str| k == "href" && self.live.is_some() && syntax == Syntax::Jsx;
        if let Some(live) = &self.live
            && syntax == Syntax::Jsx
        {
            for (k, v) in self.attrs.iter().filter(|(k, _)| default_first(k)) {
                let _ = write!(out, " {k}=\"{}\"", escape(v));
            }
            let _ = write!(out, " {{...{}}}", live.spread);
        }
        self.class_attr(out, syntax);
        for (k, v) in self.attrs.iter().filter(|(k, _)| !default_first(k)) {
            if syntax == Syntax::Jsx && *k == "src" {
                let _ = write!(out, " src={{{}}}", super::jsx::asset_binding(v));
            } else if syntax == Syntax::Jsx && *k == "tabindex" {
                // React's name, a number: TypeScript refuses `tabindex`.
                let _ = write!(out, " tabIndex={{{}}}", escape(v));
            } else {
                let _ = write!(out, " {k}=\"{}\"", escape(v));
            }
        }
        if let Some(live) = &self.live {
            live.attrs(out, syntax);
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

    /// [`Self::each`], mutably.
    pub fn each_mut(&mut self, f: &mut impl FnMut(&mut Element)) {
        f(self);
        for c in &mut self.children {
            if let Child::Element(e) = c {
                e.each_mut(f);
            }
        }
    }

    /// Every rule in the subtree, in document order. A component use's rule names
    /// its class twice, so it outranks the component root's own rule whichever
    /// stylesheet loads last.
    pub fn css(&self, out: &mut String) {
        let alts = self.alts.iter().map(|a| (&a.class, &a.style));
        for (class, style) in std::iter::once((&self.class, &self.style)).chain(alts) {
            let own;
            let style = match &self.text_class {
                Some(t) => {
                    own = t.own(style);
                    &own
                }
                None => style,
            };
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

    /// The interaction states' rules over this root's tree, in the order they
    /// stack — hover, focus, press, disabled — each live under its pseudo-class
    /// and forced by its value. Nothing for a root that is not [`Self::live`].
    pub fn states_css(&self, out: &mut String) {
        let Some(live) = &self.live else { return };
        let roots: Vec<&String> = self.classes().collect();
        for state in Interaction::ALL {
            self.each_with_depth(0, &mut |e, depth| {
                let classes: Vec<&String> = e.classes().collect();
                for s in e.states.iter().filter(|s| s.state == state) {
                    let (Some(root), Some(class)) = (roots.get(s.root_class), classes.get(s.class))
                    else {
                        continue;
                    };
                    let below = match depth {
                        0 => String::new(),
                        _ => format!(" .{class}"),
                    };
                    let _ = writeln!(
                        out,
                        "{}{below},\n.{root}[data-state=\"{}\"]{below} {{",
                        live.live_selector(state, root),
                        css_string(&s.value)
                    );
                    for (p, v) in &s.style {
                        let _ = writeln!(out, "  {p}: {v};");
                    }
                    out.push_str("}\n");
                }
            });
        }
    }

    /// [`Self::each`], with each element's depth below this one.
    fn each_with_depth(&self, depth: usize, f: &mut impl FnMut(&Element, usize)) {
        f(self, depth);
        for c in &self.children {
            if let Child::Element(e) = c {
                e.each_with_depth(depth + 1, f);
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
    /// Classes that start out taken: the text styles', which share a page with
    /// every file's own.
    pub fn reserving<'a>(names: impl IntoIterator<Item = &'a String>) -> Self {
        Classes {
            taken: names.into_iter().cloned().collect(),
            next: HashMap::new(),
        }
    }

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

    /// Every live state's selector weighs the same on every root element, so
    /// hover and focus at once, or focus on a disabled link, go by source order.
    ///
    /// flip: `Live::live_selector` writing focus with the root's class once →
    /// fails at `FocusVisible`.
    #[test]
    fn live_states_weigh_the_same() {
        // Classes, attributes and pseudo-classes, `:not()` weighing its argument.
        let weight = |s: &str| {
            s.matches('.').count() + s.matches('[').count() + s.matches(':').count()
                - s.matches(":not(").count()
        };
        for element in [RootElement::Button, RootElement::Link, RootElement::Plain] {
            let live = Live {
                prop: "state".into(),
                rest: "Default".into(),
                disabled: "disabled".into(),
                disabled_value: None,
                element,
                href: String::new(),
                spread: "rest".into(),
            };
            for state in Interaction::ALL {
                let s = live.live_selector(state, "r");
                assert_eq!(weight(&s), 4, "{state:?} on {element:?}: {s}");
            }
        }
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
