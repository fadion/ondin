//! A set's variant trees merged into one: the component is written once, each
//! element choosing its rule by the variant values, and an element only some
//! variants draw shown under those.
//!
//! Layers match across the trees by `Element::key`, which [`pair`] first sets
//! on every other tree to its counterpart's in the default's: by lineage key,
//! else by name path.

use super::dom::{Alt, Child, Element, Show, When};

/// One tree's element at a place, with the values its tree is drawn for:
/// `None` for the default's, which leads any group it is in.
type Member<'a> = (Option<&'a When>, &'a Element);

/// What [`merge`] makes of a set's trees.
pub(crate) struct Merged {
    /// The trees each drawn under their values — the variants that did not
    /// merge — then the merged tree, drawn for any other values.
    pub bodies: Vec<(When, Element)>,
    /// Each variant left out of the merge, and why.
    pub apart: Vec<(When, String)>,
}

/// `bodies` — each variant's tree with the values it is drawn for, the
/// default's last — as one tree, less any variant that does not merge with the
/// rest, which keeps a tree of its own.
///
/// Trees merge when, at every place, the elements they draw there match in all
/// but their declarations, and the layers under each come in an order every
/// tree agrees with. Variants join in order, each kept out when it would break
/// what has merged so far.
pub(crate) fn merge(mut bodies: Vec<(When, Element)>) -> Merged {
    let Some((_, default)) = bodies.pop() else {
        return Merged {
            bodies,
            apart: Vec::new(),
        };
    };
    let mut joined: Vec<(When, Element)> = Vec::new();
    let mut apart = Vec::new();
    let mut alone = Vec::new();
    for (when, tree) in bodies {
        let mut group: Vec<Member> = vec![(None, &default)];
        group.extend(joined.iter().map(|(w, t)| (Some(w), t)));
        group.push((Some(&when), &tree));
        match merged(&group) {
            Ok(_) => joined.push((when, tree)),
            Err(why) => {
                apart.push((when.clone(), why));
                alone.push((when, tree));
            }
        }
    }
    let group: Vec<Member> = std::iter::once((None, &default))
        .chain(joined.iter().map(|(w, t)| (Some(w), t)))
        .collect();
    let one = merged(&group).expect("each tree joined only where it merged");
    alone.push((Vec::new(), one));
    Merged {
        bodies: alone,
        apart,
    }
}

/// The name a report gives the layer an element draws.
pub(crate) fn layer_name(el: &Element) -> &str {
    el.key
        .as_ref()
        .map(|(k, _)| k.split('\u{0}').next().unwrap_or(k))
        .unwrap_or("its root")
}

/// Set each layer's key in `other` to its counterpart's in `default`, under a
/// paired parent: by lineage key, whichever side recorded it, else by name path;
/// a counterpart must be the only one found and not yet taken. A layer that
/// pairs with nothing keeps a key of its own — its lineage's where it recorded
/// one, so the same layer pairs across the other trees.
pub(crate) fn pair(default: &Element, other: &mut Element) {
    let layers = other
        .children
        .iter()
        .any(|c| matches!(c, Child::Element(x) if x.key.is_some()));
    if !layers {
        // Paragraphs and runs, by place.
        for (d, o) in default.children.iter().zip(&mut other.children) {
            if let (Child::Element(d), Child::Element(o)) = (d, o) {
                pair(d, o);
            }
        }
        return;
    }
    let mut used: Vec<usize> = Vec::new();
    let elements: Vec<(usize, &Element)> = default
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, d)| match d {
            Child::Element(d) => Some((i, &**d)),
            _ => None,
        })
        .collect();
    for c in &mut other.children {
        let Child::Element(o) = c else { continue };
        let Some(key) = o.key.clone() else { continue };
        // Either side's lineage key, whichever was the copy; else the name path.
        let lineage = o.lineage.map(|(k, _)| k);
        let by_key: Vec<(usize, &Element)> = elements
            .iter()
            .filter(|(_, d)| lineage.is_some() && d.lineage.map(|(k, _)| k) == lineage)
            .copied()
            .collect();
        let found = match by_key.is_empty() {
            false => by_key,
            true => elements
                .iter()
                .filter(|(_, d)| d.key.as_ref() == Some(&key))
                .copied()
                .collect(),
        };
        match found.as_slice() {
            [(i, d)] if !used.contains(i) => {
                used.push(*i);
                o.key = d.key.clone();
                pair(d, o);
            }
            // A counterpart taken or in doubt leaves `key` naming another
            // layer's, so the layer gets one of its own.
            [_, ..] => {
                if let Some(k) = lineage {
                    o.key = Some((format!("{}\u{0}{}", key.0, k.to_wire()), 0));
                }
            }
            [] => {
                if let Some((k, true)) = o.lineage {
                    o.key = Some((format!("{}\u{0}{}", key.0, k.to_wire()), 0));
                }
            }
        }
    }
}

/// The trees' elements at one place as one element: the first's, wearing each
/// other's class where its declarations differ, one class per distinct set of
/// them; or why they do not merge.
fn merged(group: &[Member]) -> Result<Element, String> {
    let (_, base) = group[0];
    for (_, e) in &group[1..] {
        same_own(base, e)?;
    }
    let mut el = base.clone();
    for (when, e) in &group[1..] {
        if e.style == el.style {
            continue;
        }
        let when = (*when).expect("only the default leads").clone();
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
    Ok(el)
}

/// Whether `b` is what `a` is but for its declarations and its children, or
/// why not.
pub(crate) fn same_own(a: &Element, b: &Element) -> Result<(), String> {
    if a.shown_by != b.shown_by {
        return Err(format!("“{}” is shown by another property", layer_name(a)));
    }
    // An inline SVG carries its class, so a merge keeps it one rule.
    let one_rule = a.markup.is_none() || a.style == b.style;
    if !one_rule || !same_kind(a, b) {
        return Err(format!(
            "“{}” is drawn as another kind of layer",
            layer_name(a)
        ));
    }
    Ok(())
}

/// Whether `b` draws what `a` does but for its declarations and its children:
/// a state's rule can change those under `a`'s class, inline SVG or not.
pub(crate) fn same_drawing(a: &Element, b: &Element) -> bool {
    a.shown_by == b.shown_by && same_kind(a, b)
}

fn same_kind(a: &Element, b: &Element) -> bool {
    let markup = match (&a.markup, &b.markup) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            let own = |e: &Element| format!("class=\"{}\"", e.class);
            *x == y.replace(&own(b), &own(a))
        }
        _ => false,
    };
    markup
        && a.tag == b.tag
        && a.attrs == b.attrs
        && a.instance == b.instance
        && a.takes_class == b.takes_class
        && a.key == b.key
}

/// The group's children merged: layers matched by key, each shown under the
/// trees that draw it; anything else — a text's paragraphs and runs — matched
/// by place, which every tree must hold alike.
fn children(group: &[Member]) -> Result<Vec<Child>, String> {
    let layers = group.iter().all(|(_, e)| {
        e.children
            .iter()
            .all(|c| matches!(c, Child::Element(x) if x.key.is_some()))
    });
    let text = || format!("the text of “{}” differs", layer_name(group[0].1));
    if !layers {
        let n = group[0].1.children.len();
        if group.iter().any(|(_, e)| e.children.len() != n) {
            return Err(text());
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
                        .collect::<Option<Vec<Member>>>()
                        .ok_or_else(text)?;
                    Ok(Child::Element(Box::new(merged(&at)?)))
                }
                c => group
                    .iter()
                    .all(|(_, e)| same_leaf(c, &e.children[i]))
                    .then(|| c.clone())
                    .ok_or_else(text),
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
    Ok(out)
}

/// Every key under the group's elements, in an order each tree's children
/// keep, or why not: two trees order two layers each other's way round.
fn order<'a>(group: &[Member<'a>]) -> Result<Vec<&'a (String, usize)>, String> {
    let mut order: Vec<&(String, usize)> = Vec::new();
    let name = |k: &(String, usize)| k.0.split('\u{0}').next().unwrap_or("").to_string();
    for &(_, e) in group {
        // Where this tree's previous layer sits in `order`.
        let mut after: Option<usize> = None;
        for c in &e.children {
            let Child::Element(x) = c else { continue };
            let Some(key) = &x.key else { continue };
            let at = match order.iter().position(|o| *o == key) {
                Some(p) if after.is_some_and(|a| p < a) => {
                    let a = after.expect("checked");
                    // This tree draws the layer before `key` first, where an
                    // earlier tree drew it after.
                    return Err(format!("“{}” moves before “{}”", name(order[a]), name(key)));
                }
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
    Ok(order)
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
        let merged = merge(bodies).bodies;
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
        let merged = merge(bodies).bodies;
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
        let apart = merge(bodies);
        assert_eq!(apart.bodies.len(), 2);
        assert_eq!(
            apart.apart,
            [(when("Large"), "“label” moves before “icon”".to_string())]
        );
        let bodies = vec![
            (when("Large"), holding("large", &["label", "badge"])),
            (Vec::new(), holding("small", &["icon", "label"])),
        ];
        let merged = merge(bodies).bodies;
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
        assert_eq!(merge(bodies).bodies.len(), 2);
    }

    /// A variant that does not merge keeps a tree of its own and the rest
    /// still merge, the one apart drawn first under its values.
    ///
    /// flip: `merge` returning every tree apart on one failure → fails at
    /// "the others merge".
    #[test]
    fn one_variant_apart_leaves_the_rest_merged() {
        let bodies = vec![
            (when("Large"), tree("large", "120px", "Go")),
            (when("Odd"), tree("odd", "80px", "Stop")),
            (Vec::new(), tree("small", "80px", "Go")),
        ];
        let m = merge(bodies);
        assert_eq!(m.bodies.len(), 2, "the others merge");
        assert_eq!(m.bodies[0].0, when("Odd"));
        assert!(m.bodies[1].0.is_empty());
        assert_eq!(m.bodies[1].1.alts.len(), 1, "Large's class");
        assert_eq!(m.apart.len(), 1);
        assert!(m.apart[0].1.contains("text"), "{:?}", m.apart);
    }

    /// A layer renamed in another variant pairs by the lineage key it
    /// recorded, so the trees merge; without the key the name keeps them apart.
    ///
    /// flip: `pair` matching by name only → fails at "paired by lineage".
    #[test]
    fn a_renamed_layer_pairs_by_its_lineage() {
        let id = |n: u64| ondin_core::NodeId::from_wire(&format!("1:{n}")).unwrap();
        let keyed = |class: &str, name: &str, own: u64, recorded: Option<u64>| {
            let mut root = Element::new("div", class.into());
            let mut e = Element::new("div", format!("{class}-icon"));
            e.key = Some((name.to_string(), 0));
            e.lineage = Some((id(recorded.unwrap_or(own)), recorded.is_some()));
            root.children.push(Child::Element(Box::new(e)));
            root
        };
        let default = keyed("small", "Icon", 5, None);
        let mut other = keyed("large", "Icon copy", 9, Some(5));
        pair(&default, &mut other);
        let m = merge(vec![(when("Large"), other), (Vec::new(), default.clone())]);
        let kids = &m.bodies[0].1.children;
        assert!(
            kids.len() == 1 && matches!(&kids[0], Child::Element(e) if e.shown_when.is_none()),
            "paired by lineage: one layer, drawn always"
        );
        let mut unkeyed = keyed("large", "Icon copy", 9, None);
        pair(&default, &mut unkeyed);
        let m = merge(vec![(when("Large"), unkeyed), (Vec::new(), default)]);
        let Child::Element(icon) = &m.bodies[0].1.children[0] else {
            panic!("an element")
        };
        assert!(icon.shown_when.is_some(), "two layers, each shown alone");
    }

    /// A layer whose counterpart another layer took by its lineage key gets a
    /// key of its own, never the taker's.
    ///
    /// flip: `pair` leaving such a layer its name-path key → fails at "one
    /// key each".
    #[test]
    fn a_taken_counterpart_leaves_a_key_of_its_own() {
        let id = |n: u64| ondin_core::NodeId::from_wire(&format!("1:{n}")).unwrap();
        let icon = |step: usize, own: u64, recorded: Option<u64>| {
            let mut e = Element::new("div", format!("icon-{own}"));
            e.key = Some(("Icon".to_string(), step));
            e.lineage = Some((id(recorded.unwrap_or(own)), recorded.is_some()));
            Child::Element(Box::new(e))
        };
        let mut default = Element::new("div", "small".into());
        default.children = vec![icon(0, 5, None), icon(1, 6, None)];
        // The copy of the second icon comes first; a new icon follows it.
        let mut other = Element::new("div", "large".into());
        other.children = vec![icon(0, 8, Some(6)), icon(1, 9, None)];
        pair(&default, &mut other);
        let keys: Vec<_> = other
            .children
            .iter()
            .filter_map(|c| match c {
                Child::Element(e) => e.key.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(keys[0], ("Icon".to_string(), 1), "paired by lineage");
        assert_ne!(keys[0], keys[1], "one key each: {keys:?}");
    }
}
