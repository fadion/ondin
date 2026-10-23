//! The plain HTML/CSS emitter: `index.html`, one `styles.css`, `assets/` and
//! `report.txt`. No components — an instance is written out as its layers.

use super::dom::{Arg, Child, Element, Show, Syntax, When};
use super::{BASE_CSS, CodeExport, CodeFile, walk};
use ondin_core::{Document, NodeId, Resolved};
use std::collections::HashMap;

/// `origins` as a page, in the order given (later on top).
pub fn html_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, false);
    let root = page.root.clone();
    let mut styles = BASE_CSS.to_string();
    page.root.css(&mut styles);
    files_of(page, root, styles)
}

/// The React and Vue page drawn as plain HTML: each use as the tree its
/// component draws for the use's arguments. The frameworks' output needs a
/// bundler to be seen; this is what it draws, for the fidelity bar.
#[doc(hidden)]
pub fn html_of_components(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, true);
    let mut styles = BASE_CSS.to_string();
    page.root.css(&mut styles);
    // Classes are unique per file, so each component's are prefixed with its name.
    let mut components = HashMap::new();
    for c in &page.components {
        let mut bodies = c.bodies.clone();
        for (_, body) in &mut bodies {
            prefix(body, &format!("{}--", c.name));
            body.css(&mut styles);
        }
        let defaults: Env = c
            .props
            .iter()
            .filter_map(|(p, _, d)| Some((p.clone(), d.clone()?)))
            .collect();
        components.insert(c.name.clone(), (bodies, defaults));
    }
    let root = drawn(&page.root, &components, &Env::new()).unwrap_or_default();
    files_of(page, root, styles)
}

/// What a component's props hold: each one's value, none of them [`Arg::Var`].
type Env = HashMap<String, Arg>;

/// Each component's trees, prefixed, and its props' defaults, by name.
type Components = HashMap<String, (Vec<(When, Element)>, Env)>;

/// `prefix` before every class under `el`, in its inline SVG's markup too.
fn prefix(el: &mut Element, prefix: &str) {
    let old = el.class.clone();
    for class in el.classes_mut() {
        class.insert_str(0, prefix);
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
            self::prefix(e, prefix);
        }
    }
}

/// Whether `when`'s values all hold in `env`.
fn holds(when: &[(String, String)], env: &Env) -> bool {
    when.iter()
        .all(|(p, v)| matches!(env.get(p), Some(Arg::Text(t)) if t == v))
}

/// `arg` read in `env`, `None` for a prop passed on that is unset there.
fn value(arg: &Arg, env: &Env) -> Option<Arg> {
    match arg {
        Arg::Var { name, fallback, .. } => env
            .get(name)
            .cloned()
            .or_else(|| fallback.as_deref().and_then(|f| value(f, env))),
        a => Some(a.clone()),
    }
}

/// `el` as drawn under `env`: hidden elements dropped, each class chosen, text
/// props filled in and every use replaced by its component's tree.
fn drawn(el: &Element, components: &Components, env: &Env) -> Option<Element> {
    if let Some(p) = &el.shown_by
        && !matches!(env.get(p), Some(Arg::Bool(true)))
    {
        return None;
    }
    let shown = match &el.shown_when {
        None => true,
        Some(Show::Under(w)) => w.iter().any(|w| holds(w, env)),
        Some(Show::Except(w)) => !w.iter().any(|w| holds(w, env)),
    };
    if !shown {
        return None;
    }
    let class = el
        .alts
        .iter()
        .find(|a| a.when.iter().any(|w| holds(w, env)))
        .map_or(&el.class, |a| &a.class)
        .clone();
    if let Some(i) = &el.instance {
        let name = match i.slot.as_ref().and_then(|s| env.get(s)) {
            Some(Arg::Component(c)) => c,
            _ => &i.component,
        };
        let (bodies, defaults) = components.get(name)?;
        let mut inner = defaults.clone();
        for (p, a) in &i.args {
            if let Some(v) = value(a, env) {
                inner.insert(p.clone(), v);
            }
        }
        let (_, body) = bodies.iter().find(|(w, _)| holds(w, &inner))?;
        let mut root = drawn(body, components, &inner)?;
        root.class = format!("{} {class}", root.class);
        return Some(root);
    }
    let mut out = Element {
        class,
        alts: Vec::new(),
        children: Vec::new(),
        shown_by: None,
        shown_when: None,
        ..el.clone()
    };
    for c in &el.children {
        match c {
            Child::Element(e) => {
                if let Some(e) = drawn(e, components, env) {
                    out.children.push(Child::Element(Box::new(e)));
                }
            }
            Child::Prop(p) => out.children.push(Child::Text(match env.get(p) {
                Some(Arg::Text(t)) => t.clone(),
                _ => String::new(),
            })),
            c => out.children.push(c.clone()),
        }
    }
    Some(out)
}

/// `index.html` around `root`, `styles.css`, and the tail every emitter writes.
fn files_of(page: walk::Page, root: Element, styles: String) -> CodeExport {
    let mut body = String::new();
    root.markup(&mut body, 1, Syntax::Html);
    let tokens_link = if page.tokens.is_some() {
        "<link rel=\"stylesheet\" href=\"tokens.css\">\n"
    } else {
        ""
    };
    let index = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Ondin export</title>\n{tokens_link}<link rel=\"stylesheet\" href=\"styles.css\">\n\
         </head>\n<body>\n{body}</body>\n</html>\n"
    );
    let files = vec![
        CodeFile {
            path: "index.html".into(),
            bytes: index.into_bytes(),
        },
        CodeFile {
            path: "styles.css".into(),
            bytes: styles.into_bytes(),
        },
    ];
    super::finish(files, page)
}
