//! A marked set's interaction states as CSS — a marked set is one whose
//! property ties values to browser states — each state variant's tree diffed
//! against rest's — the variant with the same values of every other property —
//! and written as rules under `:hover`, `:focus-visible`, `:active` and
//! `:disabled`, each also forced by the state prop (`data-state`).
//!
//! The variants that are not states merge as any set's do (`merge::merge`); a
//! state is a diff against the merged tree as its rest draws it. A layer drawn
//! another way than rest's counterpart — another kind, a picture that differs,
//! another use — is swapped: rest's hidden, the state's shown. A state whose
//! layers do not line up with rest's — other text, another order — keeps a tree
//! of its own behind the prop alone, and is reported.

use super::Report;
use super::dom::{Alt, Child, Classes, Element, Show, StateStyle, When};
use super::merge;
use ondin_core::variant::Interaction;
use std::collections::BTreeMap;

/// One variant's tree, with the values its tree is drawn for and its own.
pub(crate) struct Body {
    pub when: When,
    pub values: Vec<String>,
    pub tree: Element,
}

/// What [`write()`] needs of the set.
pub(crate) struct Marked<'a> {
    /// The component's name, for the report.
    pub component: &'a str,
    /// The prop forcing a state, as code names it.
    pub prop: &'a str,
    /// The interaction-state property's place among the values.
    pub at: usize,
    pub rest: &'a str,
    pub ties: &'a BTreeMap<String, Interaction>,
}

/// Declarations a state never writes on the root: a use's placement and size
/// must win.
const PLACEMENT: &[&str] = &[
    "position",
    "left",
    "top",
    "right",
    "bottom",
    "inset",
    "width",
    "height",
    "min-width",
    "min-height",
    "max-width",
    "max-height",
    "aspect-ratio",
    "margin",
    "flex",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "align-self",
    "justify-self",
    "grid-column",
    "grid-row",
    "grid-area",
    "transform",
];

/// Declarations, each on the element of the merged tree at its path.
type Changes = Vec<(Vec<usize>, Vec<(&'static str, String)>)>;

/// One state's declarations on one element of the merged tree, by its path.
struct Entry {
    state: Interaction,
    value: String,
    path: Vec<usize>,
    style: Vec<(&'static str, String)>,
}

/// `bodies`, the default's last and every tree paired (`merge::pair`), as the
/// trees a component draws: the states that keep their own, the variants that
/// do not merge, then the merged tree carrying the states' rules.
pub(crate) fn write(
    bodies: Vec<Body>,
    m: &Marked,
    classes: &mut Classes,
    report: &mut Report,
) -> Vec<(When, Element)> {
    let state_of = |values: &[String]| {
        values
            .get(m.at)
            .filter(|v| v.as_str() != m.rest)
            .and_then(|v| m.ties.get(v).copied())
    };
    let (states, base): (Vec<Body>, Vec<Body>) = bodies
        .into_iter()
        .partition(|b| state_of(&b.values).is_some());
    let base_values: Vec<(When, Vec<String>)> = base
        .iter()
        .map(|b| (b.when.clone(), b.values.clone()))
        .collect();
    let merged = merge::merge(base.into_iter().map(|b| (b.when, b.tree)).collect());
    let mut out = merged.bodies;
    let mut one = out.pop().expect("the merged tree").1;
    // The variants that merged, and those kept apart with why, by values.
    let (joined, apart): (Vec<(When, Vec<String>)>, Vec<_>) = base_values
        .into_iter()
        .partition(|(w, _)| !merged.apart.iter().any(|(a, _)| a == w));
    let apart: Vec<(Vec<String>, &str)> = apart
        .into_iter()
        .filter_map(|(w, v)| {
            let why = merged.apart.iter().find(|(a, _)| *a == w)?;
            Some((v, why.1.as_str()))
        })
        .collect();

    // Line each state up with rest's tree, adding the layers only it draws.
    let mut passed: Vec<(Body, When, Interaction)> = Vec::new();
    let mut own: Vec<(When, Element)> = Vec::new();
    for s in states {
        let state = state_of(&s.values).expect("a state");
        let mut rest_values = s.values.clone();
        rest_values[m.at] = m.rest.to_string();
        let rest = joined.iter().find(|(_, v)| *v == rest_values);
        let rest_name = ondin_core::variant::derived_name(&rest_values);
        let why = match rest {
            None => Err(
                match apart
                    .iter()
                    .find(|(v, _)| *v == rest_values)
                    .map(|(_, w)| w)
                {
                    Some(w) => format!("{rest_name} does not merge with the other variants ({w})"),
                    None => format!("no variant is {rest_name} at rest"),
                },
            ),
            Some((r, _)) => line_up(&mut one, &s.tree, r, &joined, classes),
        };
        match why {
            Ok(()) => {
                let r = rest.expect("lined up").0.clone();
                passed.push((s, r, state));
            }
            Err(why) => {
                report.approximate(format!(
                    "{} · {}: {why}, so {} is written only behind the `{}` prop",
                    m.component,
                    ondin_core::variant::derived_name(&s.values),
                    state.selector(),
                    m.prop
                ));
                own.push((s.when, s.tree));
            }
        }
    }

    // What each state changes, now that every tree's layers are in place.
    let mut entries: Vec<(When, Entry)> = Vec::new();
    for (s, r, state) in &passed {
        let value = s.values[m.at].clone();
        let mut found = Vec::new();
        diff(&one, &s.tree, r, &mut Vec::new(), &mut found);
        // The root always gets a focus rule, whatever else the state changes.
        if *state == Interaction::FocusVisible && !found.iter().any(|(p, _)| p.is_empty()) {
            found.insert(0, (Vec::new(), Vec::new()));
        }
        for (path, mut style) in found {
            if path.is_empty() {
                let moved: Vec<&str> = style
                    .iter()
                    .filter(|(p, _)| PLACEMENT.contains(p))
                    .map(|(p, _)| *p)
                    .collect();
                if !moved.is_empty() {
                    report.approximate(format!(
                        "{} · {}: {}; drawn at rest's size and place",
                        m.component,
                        ondin_core::variant::derived_name(&s.values),
                        root_change(&one, &s.tree, r)
                    ));
                }
                style.retain(|(p, _)| !PLACEMENT.contains(p));
                // The state draws its own focus, never the browser's ring too.
                if *state == Interaction::FocusVisible {
                    style.push(("outline", "none".into()));
                }
            }
            if !style.is_empty() {
                entries.push((
                    r.clone(),
                    Entry {
                        state: *state,
                        value: value.clone(),
                        path,
                        style,
                    },
                ));
            }
        }
    }

    split_roots(&mut one, &joined, &entries, classes);
    for (r, e) in entries {
        let root_class = class_under(&one, &r);
        let Some(el) = at_path(&mut one, &e.path) else {
            continue;
        };
        let class = class_under(el, &r);
        el.states.push(StateStyle {
            state: e.state,
            value: e.value,
            root_class,
            class,
            style: e.style,
        });
    }

    let written: Vec<&str> = Interaction::ALL
        .iter()
        .filter_map(|s| {
            m.ties
                .iter()
                .find(|(_, t)| *t == s)
                .map(|(v, _)| v.as_str())
        })
        .collect();
    let selectors: Vec<&str> = Interaction::ALL
        .iter()
        .filter(|s| m.ties.values().any(|t| t == *s))
        .map(|s| s.selector())
        .collect();
    if !written.is_empty() {
        report.state(format!(
            "{}: {} written as {}; `{}` forces one",
            m.component,
            and_list(&written),
            and_list(&selectors),
            m.prop
        ));
    }

    own.extend(out);
    own.push((Vec::new(), one));
    own
}

/// `a, b and c`.
fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Whether `el` is drawn under the variant `when`.
fn shown_under(el: &Element, when: &When) -> bool {
    match &el.shown_when {
        None => true,
        Some(Show::Under(w)) => w.contains(when),
        Some(Show::Except(w)) => !w.contains(when),
    }
}

/// The place in [`Element::classes`] of the class `el` wears under `when`.
fn class_under(el: &Element, when: &When) -> usize {
    el.alts
        .iter()
        .position(|a| a.when.contains(when))
        .map_or(0, |i| i + 1)
}

/// The declarations `el` draws under `when`.
fn style_under<'a>(el: &'a Element, when: &When) -> &'a [(&'static str, String)] {
    match class_under(el, when) {
        0 => &el.style,
        i => &el.alts[i - 1].style,
    }
}

fn at_path<'a>(el: &'a mut Element, path: &[usize]) -> Option<&'a mut Element> {
    let Some((first, rest)) = path.split_first() else {
        return Some(el);
    };
    match el.children.get_mut(*first)? {
        Child::Element(e) => at_path(e, rest),
        _ => None,
    }
}

/// Check `s`'s tree against `m`'s as the variant `r` draws it, and add to `m`
/// each layer only `s` draws, hidden but where the state shows it; or why the
/// two do not line up.
fn line_up(
    m: &mut Element,
    s: &Element,
    r: &When,
    joined: &[(When, Vec<String>)],
    classes: &mut Classes,
) -> Result<(), String> {
    check(m, s, r)?;
    insert(m, s, r, joined, classes);
    Ok(())
}

/// [`line_up`]'s check, changing nothing.
fn check(m: &Element, s: &Element, r: &When) -> Result<(), String> {
    if !merge::same_drawing(m, s) {
        return Err(format!(
            "“{}” is drawn as another kind of layer",
            merge::layer_name(m)
        ));
    }
    let layers = s
        .children
        .iter()
        .any(|c| matches!(c, Child::Element(x) if x.key.is_some()));
    if !layers {
        let shown: Vec<&Child> = m
            .children
            .iter()
            .filter(|c| !matches!(c, Child::Element(x) if !shown_under(x, r)))
            .collect();
        let text = || format!("the text of “{}” differs", merge::layer_name(m));
        if shown.len() != s.children.len() {
            return Err(text());
        }
        for (a, b) in shown.into_iter().zip(&s.children) {
            match (a, b) {
                (Child::Element(a), Child::Element(b)) => check(a, b, r)?,
                (Child::Text(a), Child::Text(b)) | (Child::Prop(a), Child::Prop(b)) if a == b => {}
                (Child::Break, Child::Break) => {}
                _ => return Err(text()),
            }
        }
        return Ok(());
    }
    // The layers both draw, in rest's order, as `s` meets them.
    let mut last: Option<(usize, &Element)> = None;
    for c in &s.children {
        let Child::Element(x) = c else { continue };
        let Some(at) = paired(m, x, r) else { continue };
        let Child::Element(y) = &m.children[at] else {
            unreachable!("paired finds elements")
        };
        if let Some((before, b)) = last
            && at < before
        {
            return Err(format!(
                "“{}” moves before “{}”",
                merge::layer_name(b),
                merge::layer_name(x)
            ));
        }
        check(y, x, r)?;
        last = Some((at, y));
    }
    Ok(())
}

/// Where in `m`'s children the layer `x` is, among those `r` draws. A
/// counterpart drawn another way — another tag, picture, use or binding — is
/// none: rest's is hidden and `x` is added in its place.
fn paired(m: &Element, x: &Element, r: &When) -> Option<usize> {
    let key = x.key.as_ref()?;
    m.children.iter().position(|c| {
        matches!(c, Child::Element(y) if y.key.as_ref() == Some(key)
            && shown_under(y, r)
            && merge::same_drawing(y, x))
    })
}

/// [`line_up`]'s additions: each layer of `s` with no counterpart in `m`,
/// after the counterpart of the layer before it, its rule `display: none` and
/// drawn only under `r`.
fn insert(
    m: &mut Element,
    s: &Element,
    r: &When,
    joined: &[(When, Vec<String>)],
    classes: &mut Classes,
) {
    let layers = s
        .children
        .iter()
        .any(|c| matches!(c, Child::Element(x) if x.key.is_some()));
    if !layers {
        let at: Vec<usize> = (0..m.children.len())
            .filter(|i| !matches!(&m.children[*i], Child::Element(x) if !shown_under(x, r)))
            .collect();
        for (i, c) in at.into_iter().zip(&s.children) {
            if let (Child::Element(a), Child::Element(b)) = (&mut m.children[i], c) {
                insert(a, b, r, joined, classes);
            }
        }
        return;
    }
    let mut next = 0;
    for c in &s.children {
        let Child::Element(x) = c else { continue };
        match paired(m, x, r) {
            Some(at) => {
                if let Child::Element(y) = &mut m.children[at] {
                    insert(y, x, r, joined, classes);
                }
                next = at + 1;
            }
            None => {
                let mut added = (**x).clone();
                rename(&mut added, classes);
                added.set("display", "none");
                added.shown_when = Some(match r.is_empty() {
                    false => Show::Under(vec![r.clone()]),
                    // The default's: every other variant left out.
                    true => Show::Except(
                        joined
                            .iter()
                            .map(|(w, _)| w.clone())
                            .filter(|w| !w.is_empty())
                            .collect(),
                    ),
                });
                m.children.insert(next, Child::Element(Box::new(added)));
                next += 1;
            }
        }
    }
}

/// A layer copied from a state's tree into the merged one takes classes of
/// its own in the component's file, its inline SVG's too.
fn rename(el: &mut Element, classes: &mut Classes) {
    let old = el.class.clone();
    for class in el.classes_mut() {
        *class = classes.mint(class, "layer");
    }
    if let Some(m) = &mut el.markup {
        *m = m.replacen(
            &format!("class=\"{old}\""),
            &format!("class=\"{}\"", el.class),
            1,
        );
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            rename(e, classes);
        }
    }
}

/// What each element of `m` drawn under `r` needs to look as `s` does: the
/// declarations that changed (one gone written `unset`, or for `display`
/// `revert`), and `display: none` on a layer only rest draws.
fn diff(m: &Element, s: &Element, r: &When, path: &mut Vec<usize>, out: &mut Changes) {
    let mine = style_under(m, r);
    let mut style: Vec<(&'static str, String)> = Vec::new();
    for (p, v) in &s.style {
        if !mine.iter().any(|(q, w)| q == p && w == v) {
            style.push((*p, v.clone()));
        }
    }
    for (p, _) in mine {
        if !s.style.iter().any(|(q, _)| q == p) {
            let gone = if *p == "display" { "revert" } else { "unset" };
            style.push((*p, gone.into()));
        }
    }
    if !style.is_empty() {
        out.push((path.clone(), style));
    }
    let layers = s
        .children
        .iter()
        .any(|c| matches!(c, Child::Element(x) if x.key.is_some()));
    let mut met = Vec::new();
    if layers {
        for c in &s.children {
            let Child::Element(x) = c else { continue };
            if let Some(at) = paired(m, x, r) {
                met.push(at);
                if let Child::Element(y) = &m.children[at] {
                    path.push(at);
                    diff(y, x, r, path, out);
                    path.pop();
                }
            }
        }
        for (i, c) in m.children.iter().enumerate() {
            if let Child::Element(y) = c
                && y.key.is_some()
                && shown_under(y, r)
                && !met.contains(&i)
            {
                path.push(i);
                out.push((path.clone(), vec![("display", "none".into())]));
                path.pop();
            }
        }
    } else {
        let at: Vec<usize> = (0..m.children.len())
            .filter(|i| !matches!(&m.children[*i], Child::Element(x) if !shown_under(x, r)))
            .collect();
        for (i, c) in at.into_iter().zip(&s.children) {
            if let (Child::Element(y), Child::Element(x)) = (&m.children[i], c) {
                path.push(i);
                diff(y, x, r, path, out);
                path.pop();
            }
        }
    }
}

/// What a state did to the root's box, as the report says it.
fn root_change(m: &Element, s: &Element, r: &When) -> String {
    let px = |style: &[(&'static str, String)], p: &str| {
        style
            .iter()
            .find(|(q, _)| *q == p)
            .and_then(|(_, v)| v.strip_suffix("px")?.parse::<f64>().ok())
    };
    let rest = style_under(m, r);
    let (w0, h0, w1, h1) = (
        px(rest, "width"),
        px(rest, "height"),
        px(&s.style, "width"),
        px(&s.style, "height"),
    );
    match (w0, h0, w1, h1) {
        (Some(w0), Some(h0), Some(w1), Some(h1)) if w0 != w1 || h0 != h1 => {
            let d = if (w1 - w0).abs() >= (h1 - h0).abs() {
                w1 - w0
            } else {
                h1 - h0
            };
            let n = super::css::px(d.abs());
            let n = n.trim_end_matches("px");
            match d < 0.0 {
                true => format!("the root is {n} px smaller than at rest"),
                false => format!("the root is {n} px larger than at rest"),
            }
        }
        _ => "the root's size or place differs from rest's".to_string(),
    }
}

/// Give each variant its own root class where variants sharing one would need
/// different state rules: a rule names the element by its root's class, so two
/// variants wearing the same root class would share each other's states.
fn split_roots(
    one: &mut Element,
    joined: &[(When, Vec<String>)],
    entries: &[(When, Entry)],
    classes: &mut Classes,
) {
    let signature = |w: &When| {
        let mut sig: Vec<String> = entries
            .iter()
            .filter(|(r, _)| r == w)
            .map(|(_, e)| format!("{:?} {:?} {:?}", e.state, e.path, e.style))
            .collect();
        sig.sort();
        sig
    };
    let mut groups: BTreeMap<usize, Vec<&When>> = BTreeMap::new();
    for (w, _) in joined {
        groups.entry(class_under(one, w)).or_default().push(w);
    }
    for (_, members) in groups {
        let first = signature(members[0]);
        if members.iter().all(|w| signature(w) == first) {
            continue;
        }
        // The default keeps the class, as it wears it with no condition.
        let keeper = members.iter().position(|w| w.is_empty()).unwrap_or(0);
        for (i, w) in members.iter().enumerate() {
            if i == keeper {
                continue;
            }
            let at = class_under(one, w);
            let style = style_under(one, w).to_vec();
            let base = if at == 0 {
                one.class.clone()
            } else {
                one.alts[at - 1].class.clone()
            };
            if at > 0 {
                one.alts[at - 1].when.retain(|x| x != *w);
            }
            one.alts.push(Alt {
                when: vec![(*w).clone()],
                class: classes.mint(&base, "layer"),
                style,
            });
        }
        one.alts.retain(|a| !a.when.is_empty());
    }
}
