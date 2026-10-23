//! A set's variant trees merged into one: where every variant draws the same
//! elements, the component is written once and each element chooses its rule by
//! the variant values.

use super::dom::{Alt, Child, Element, When};

/// `bodies` — each variant's tree with the values it is drawn for, the
/// default's last — as one tree when every tree matches the default's in all
/// but its elements' declarations; otherwise unchanged.
pub(crate) fn merge(bodies: Vec<(When, Element)>) -> Vec<(When, Element)> {
    let Some(((_, default), others)) = bodies.split_last() else {
        return bodies;
    };
    if others.is_empty() || !others.iter().all(|(_, t)| same_shape(default, t)) {
        return bodies;
    }
    let mut merged = default.clone();
    let trees: Vec<(&When, &Element)> = others.iter().map(|(w, t)| (w, t)).collect();
    fold(&mut merged, &trees);
    vec![(Vec::new(), merged)]
}

/// Whether `b` draws what `a` does but for the declarations of its elements.
fn same_shape(a: &Element, b: &Element) -> bool {
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
        && a.children.len() == b.children.len()
        && a.children.iter().zip(&b.children).all(|c| match c {
            (Child::Element(x), Child::Element(y)) => same_shape(x, y),
            (Child::Text(x), Child::Text(y)) | (Child::Prop(x), Child::Prop(y)) => x == y,
            (Child::Break, Child::Break) => true,
            _ => false,
        })
}

/// Gives `el`, the default's element, the class of each of `trees`' elements at
/// its place whose declarations differ, one class per distinct set of them.
fn fold(el: &mut Element, trees: &[(&When, &Element)]) {
    for (when, t) in trees {
        if t.style == el.style {
            continue;
        }
        match el.alts.iter_mut().find(|a| a.style == t.style) {
            Some(a) => a.when.push((*when).clone()),
            None => el.alts.push(Alt {
                when: vec![(*when).clone()],
                class: t.class.clone(),
                style: t.style.clone(),
            }),
        }
    }
    for (i, child) in el.children.iter_mut().enumerate() {
        let Child::Element(c) = child else { continue };
        let at: Vec<(&When, &Element)> = trees
            .iter()
            .filter_map(|(w, t)| match &t.children[i] {
                Child::Element(e) => Some((*w, &**e)),
                _ => None,
            })
            .collect();
        fold(c, &at);
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
