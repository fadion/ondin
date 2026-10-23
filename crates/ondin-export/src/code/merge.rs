//! A set's variant trees merged into one: the component is written once, each
//! element choosing its rule by the variant values, and an element only some
//! variants draw shown under those.
//!
//! Layers match across the trees by their step in the name path, as the model
//! matches a variant's layers (`Element::key`).

use super::dom::{Alt, Child, Element, Show, When};

/// One tree's element at a place, with the values its tree is drawn for:
/// `None` for the default's, which leads any group it is in.
type Member<'a> = (Option<&'a When>, &'a Element);

/// `bodies` — each variant's tree with the values it is drawn for, the
/// default's last — as one tree when the trees merge; otherwise unchanged.
///
/// They merge when, at every place, the elements the trees draw there match in
/// all but their declarations, and the layers under each come in an order every
/// tree agrees with.
pub(crate) fn merge(bodies: Vec<(When, Element)>) -> Vec<(When, Element)> {
    let one = {
        let Some(((_, default), others)) = bodies.split_last() else {
            return bodies;
        };
        if others.is_empty() {
            return bodies;
        }
        let group: Vec<Member> = std::iter::once((None, default))
            .chain(others.iter().map(|(w, t)| (Some(w), t)))
            .collect();
        merged(&group)
    };
    match one {
        Some(el) => vec![(Vec::new(), el)],
        None => bodies,
    }
}

/// The trees' elements at one place as one element: the first's, wearing each
/// other's class where its declarations differ, one class per distinct set of
/// them.
fn merged(group: &[Member]) -> Option<Element> {
    let (_, base) = group[0];
    if !group[1..].iter().all(|(_, e)| same_own(base, e)) {
        return None;
    }
    let mut el = base.clone();
    for (when, e) in &group[1..] {
        if e.style == el.style {
            continue;
        }
        let when = (*when)?.clone();
        match el.alts.iter_mut().find(|a| a.style == e.style) {
            Some(a) => a.when.push(when),
            None => el.alts.push(Alt {
                when: vec![when],
                class: e.class.clone(),
                style: e.style.clone(),
            }),
        }
    }
    el.children = children(group)?;
    Some(el)
}

/// Whether `b` is what `a` is but for its declarations and its children.
fn same_own(a: &Element, b: &Element) -> bool {
    let markup = match (&a.markup, &b.markup) {
        (None, None) => true,
        // An inline SVG carries its class, so it keeps one rule.
        (Some(x), Some(y)) => {
            let own = |e: &Element| format!("class=\"{}\"", e.class);
            a.style == b.style && *x == y.replace(&own(b), &own(a))
        }
        _ => false,
    };
    markup
        && a.tag == b.tag
        && a.attrs == b.attrs
        && a.instance == b.instance
        && a.shown_by == b.shown_by
        && a.takes_class == b.takes_class
        && a.key == b.key
}

/// The group's children merged: layers matched by key, each shown under the
/// trees that draw it; anything else — a text's paragraphs and runs — matched
/// by place, which every tree must hold alike.
fn children(group: &[Member]) -> Option<Vec<Child>> {
    let layers = group.iter().all(|(_, e)| {
        e.children
            .iter()
            .all(|c| matches!(c, Child::Element(x) if x.key.is_some()))
    });
    if !layers {
        let n = group[0].1.children.len();
        if group.iter().any(|(_, e)| e.children.len() != n) {
            return None;
        }
        return (0..n)
            .map(|i| match &group[0].1.children[i] {
                Child::Element(_) => {
                    let at = group
                        .iter()
                        .map(|(w, e)| match &e.children[i] {
                            Child::Element(x) => Some((*w, &**x)),
                            _ => None,
                        })
                        .collect::<Option<Vec<Member>>>()?;
                    Some(Child::Element(Box::new(merged(&at)?)))
                }
                c => group
                    .iter()
                    .all(|(_, e)| same_leaf(c, &e.children[i]))
                    .then(|| c.clone()),
            })
            .collect();
    }
    let mut out = Vec::new();
    for key in order(group)? {
        let havers: Vec<Member> = group
            .iter()
            .filter_map(|(w, e)| {
                e.children.iter().find_map(|c| match c {
                    Child::Element(x) if x.key.as_ref() == Some(key) => Some((*w, &**x)),
                    _ => None,
                })
            })
            .collect();
        let mut el = merged(&havers)?;
        if havers.len() < group.len() {
            el.shown_when = Some(match havers[0].0 {
                // The default's tree draws it: shown but where a tree without it
                // is chosen. Whens are disjoint, as no two variants share values.
                None => Show::Except(
                    group
                        .iter()
                        .filter(|(w, _)| !havers.iter().any(|(h, _)| h == w))
                        .filter_map(|(w, _)| w.cloned())
                        .collect(),
                ),
                Some(_) => Show::Under(havers.iter().filter_map(|(w, _)| w.cloned()).collect()),
            });
        }
        out.push(Child::Element(Box::new(el)));
    }
    Some(out)
}

/// Every key under the group's elements, in an order each tree's children
/// keep, or `None` when two trees order two layers each other's way round.
fn order<'a>(group: &[Member<'a>]) -> Option<Vec<&'a (String, usize)>> {
    let mut order: Vec<&(String, usize)> = Vec::new();
    for &(_, e) in group {
        // Where this tree's previous layer sits in `order`.
        let mut after: Option<usize> = None;
        for c in &e.children {
            let Child::Element(x) = c else { continue };
            let Some(key) = &x.key else { continue };
            let at = match order.iter().position(|o| *o == key) {
                Some(p) if after.is_some_and(|a| p < a) => return None,
                Some(p) => p,
                None => {
                    let p = after.map_or(0, |a| a + 1);
                    order.insert(p, key);
                    p
                }
            };
            after = Some(at);
        }
    }
    Some(order)
}

/// Whether two children that are not elements are alike.
fn same_leaf(a: &Child, b: &Child) -> bool {
    match (a, b) {
        (Child::Text(x), Child::Text(y)) | (Child::Prop(x), Child::Prop(y)) => x == y,
        (Child::Break, Child::Break) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::dom::Syntax;

    fn tree(class: &str, width: &str, text: &str) -> Element {
        let mut root = Element::new("div", class.into());
        root.set("width", width);
        root.takes_class = true;
        let mut p = Element::new("p", format!("{class}-p"));
        p.children.push(Child::Text(text.into()));
        root.children.push(Child::Element(Box::new(p)));
        root
    }

    fn when(size: &str) -> When {
        vec![("size".into(), size.into())]
    }

    /// Variants drawing alike share one class however many there are, worn
    /// under any of their values; a variant drawing the default's rule wears the
    /// default's class.
    /// flip: `fold` pushing a class per tree → fails at "one alternative".
    #[test]
    fn variants_drawing_alike_share_one_class() {
        let bodies = vec![
            (when("Large"), tree("large", "120px", "Go")),
            (when("Huge"), tree("huge", "120px", "Go")),
            (when("Same"), tree("same", "80px", "Go")),
            (Vec::new(), tree("small", "80px", "Go")),
        ];
        let merged = merge(bodies);
        assert_eq!(merged.len(), 1, "one tree");
        let root = &merged[0].1;
        assert_eq!(root.alts.len(), 1, "one alternative: {:?}", root.alts);
        assert_eq!(root.alts[0].when, vec![when("Large"), when("Huge")]);
        let mut out = String::new();
        root.markup(&mut out, 0, Syntax::Jsx);
        assert!(
            out.starts_with(
                "<div className={`${size === \"Large\" || size === \"Huge\" ? styles.large : styles.small} ${className ?? \"\"}`}>"
            ),
            "{out}"
        );
        let mut css = String::new();
        root.css(&mut css);
        assert!(!css.contains(".huge"), "{css}");
    }

    /// A frame holding a layer per name, each keyed as its first of that name.
    fn holding(class: &str, layers: &[&str]) -> Element {
        let mut root = Element::new("div", class.into());
        for name in layers {
            let mut e = Element::new("div", format!("{class}-{name}"));
            e.key = Some((name.to_string(), 0));
            root.children.push(Child::Element(Box::new(e)));
        }
        root
    }

    /// A layer the default draws and another variant lacks is shown under all
    /// but that variant, since the default's tree is drawn for any other values.
    /// flip: `children` writing `Show::Under` of the trees holding it → fails at
    /// "all but Large".
    #[test]
    fn a_layer_the_default_draws_is_shown_but_where_a_tree_lacks_it() {
        let bodies = vec![
            (when("Large"), holding("large", &["label"])),
            (when("Huge"), holding("huge", &["icon", "label"])),
            (Vec::new(), holding("small", &["icon", "label"])),
        ];
        let merged = merge(bodies);
        assert_eq!(merged.len(), 1);
        let Child::Element(icon) = &merged[0].1.children[0] else {
            panic!("an element")
        };
        assert_eq!(
            icon.shown_when,
            Some(Show::Except(vec![when("Large")])),
            "all but Large"
        );
        let mut out = String::new();
        merged[0].1.markup(&mut out, 0, Syntax::Vue);
        assert!(
            out.contains("<div class=\"small-icon\" v-if=\"!(size === 'Large')\">"),
            "{out}"
        );
    }

    /// Two layers each tree orders its own way round keep the trees apart: no
    /// one order draws both.
    #[test]
    fn layers_ordered_each_way_round_keep_the_trees_apart() {
        let bodies = vec![
            (when("Large"), holding("large", &["label", "icon"])),
            (Vec::new(), holding("small", &["icon", "label"])),
        ];
        assert_eq!(merge(bodies).len(), 2);
        let bodies = vec![
            (when("Large"), holding("large", &["label", "badge"])),
            (Vec::new(), holding("small", &["icon", "label"])),
        ];
        let merged = merge(bodies);
        let keys: Vec<_> = merged[0]
            .1
            .children
            .iter()
            .map(|c| match c {
                Child::Element(e) => e.key.clone().unwrap().0,
                _ => panic!("an element"),
            })
            .collect();
        assert_eq!(keys, ["icon", "label", "badge"], "each tree's order kept");
    }

    /// A variant whose text differs keeps the trees apart.
    #[test]
    fn different_text_keeps_the_trees_apart() {
        let bodies = vec![
            (when("Large"), tree("large", "120px", "Go")),
            (Vec::new(), tree("small", "80px", "Stop")),
        ];
        assert_eq!(merge(bodies).len(), 2);
    }
}
