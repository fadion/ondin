//! The Vue emitter: single-file components in TypeScript with scoped styles.
//!
//! `App.vue` is the page; each main an instance on it uses, or the set it is a
//! variant of, becomes `components/<Name>.vue`: its variant properties string
//! unions choosing a variant's tree by `v-if`, its boolean and text properties
//! typed props, each defaulting to the default variant's value. A class on a
//! component falls through to its root, and the user's scoped rule reaches that
//! root, so the user places it. `base.css` holds the page reset every file
//! imports, and `tokens.css` the variables.

use super::dom::{Arg, Child, Element, Syntax, escape};
use super::jsx::js_string;
use super::walk::{self, Component};
use super::{BASE_CSS, CodeExport, CodeFile};
use ondin_core::{Document, NodeId, Resolved};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// `origins` as `App`, in the order given (later on top).
pub fn vue_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, true);
    let tokens = page.tokens.is_some();
    let app = Component::page(page.root.clone());
    let all: Vec<&Component> = std::iter::once(&app).chain(&page.components).collect();

    // A used component's root carries its user's scope as well as its own, and
    // its user's class as well as its own, so each file's classes are settled
    // against the files it uses in two passes.
    let roots: HashMap<&str, Vec<&str>> = all
        .iter()
        .map(|c| {
            let classes = c.bodies.iter().map(|(_, e)| e.class.as_str()).collect();
            (c.name.as_str(), classes)
        })
        .collect();
    let mut trees: Vec<Vec<Element>> = all
        .iter()
        .map(|c| {
            let mut bodies: Vec<Element> = c.bodies.iter().map(|(_, e)| e.clone()).collect();
            let (uses, _) = super::references(bodies.iter());
            let taken: HashSet<String> = uses
                .iter()
                .filter_map(|u| roots.get(u.as_str()))
                .flatten()
                .map(|c| c.to_string())
                .collect();
            avoid(&mut bodies, &taken);
            bodies
        })
        .collect();
    let classes: HashMap<String, HashSet<String>> = all
        .iter()
        .zip(&trees)
        .map(|(c, bodies)| (c.name.clone(), classes_in(bodies)))
        .collect();
    for bodies in &mut trees {
        let mut mine = classes_in(bodies);
        for body in bodies.iter_mut() {
            keep_uses_apart(body, &classes, &roots, &mut mine);
        }
    }

    let mut files = Vec::new();
    for (i, (c, bodies)) in all.iter().zip(trees).enumerate() {
        let (path, up) = match i {
            0 => ("App.vue".to_string(), "./"),
            _ => (format!("components/{}.vue", c.name), "../"),
        };
        files.push(CodeFile {
            path,
            bytes: sfc(c, bodies, tokens, up).into_bytes(),
        });
        if i == 0 {
            files.push(CodeFile {
                path: "base.css".into(),
                bytes: BASE_CSS.as_bytes().to_vec(),
            });
        }
    }
    super::finish(files, page)
}

/// Every class in a file's trees.
fn classes_in(bodies: &[Element]) -> HashSet<String> {
    let mut all = HashSet::new();
    for body in bodies {
        body.each(&mut |e| {
            all.insert(e.class.clone());
        });
    }
    all
}

/// Renames each use in `el`'s tree whose class is also a class of the file it
/// uses, where that file's own rule would reach the root the class lands on.
/// Its root's own class is harmless on a component of one tree, which wears it
/// anyway. `mine` is the file's classes, which a new name joins.
fn keep_uses_apart(
    el: &mut Element,
    classes: &HashMap<String, HashSet<String>>,
    roots: &HashMap<&str, Vec<&str>>,
    mine: &mut HashSet<String>,
) {
    if let Some(i) = &el.instance {
        let theirs = classes.get(&i.component);
        let own_root = roots
            .get(i.component.as_str())
            .is_some_and(|r| r.len() == 1 && r[0] == el.class);
        if let Some(theirs) = theirs.filter(|t| !own_root && t.contains(&el.class)) {
            let base = el.class.clone();
            let mut n = 2;
            while mine.contains(&format!("{base}-{n}")) || theirs.contains(&format!("{base}-{n}")) {
                n += 1;
            }
            el.class = format!("{base}-{n}");
            mine.insert(el.class.clone());
        }
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            keep_uses_apart(e, classes, roots, mine);
        }
    }
}

/// One `.vue` file from `c` and its trees, their classes settled. `up` reaches
/// the export's top folder from the file.
fn sfc(c: &Component, mut bodies: Vec<Element>, tokens: bool, up: &str) -> String {
    let is_app = c.name == "App";
    let (uses, _) = super::references(bodies.iter());
    // A chain of `v-if`s is one root to Vue, so a class still falls through. The
    // default variant's tree is last, with no condition.
    for (i, (body, (when, _))) in bodies.iter_mut().zip(&c.bodies).enumerate() {
        if when.is_empty() {
            if i > 0 {
                body.branch = Some("v-else".into());
            }
            continue;
        }
        let cond: Vec<String> = when
            .iter()
            .map(|(p, v)| format!("{p} === {}", single_quoted(v)))
            .collect();
        let directive = if i == 0 { "v-if" } else { "v-else-if" };
        body.branch = Some(format!("{directive}=\"{}\"", escape(&cond.join(" && "))));
    }

    let mut out = String::from("<script setup lang=\"ts\">\n");
    let _ = writeln!(out, "import \"{up}base.css\";");
    if tokens {
        let _ = writeln!(out, "import \"{up}tokens.css\";");
    }
    for u in &uses {
        let from = if is_app {
            format!("./components/{u}.vue")
        } else {
            format!("./{u}.vue")
        };
        let _ = writeln!(out, "import {u} from \"{from}\";");
    }
    if !c.props.is_empty() || !c.choices.is_empty() {
        out.push_str("\nwithDefaults(\n  defineProps<{\n");
        for (p, values, _) in &c.choices {
            let union: Vec<String> = values.iter().map(|v| js_string(v)).collect();
            let _ = writeln!(out, "    {p}?: {};", union.join(" | "));
        }
        for (p, d) in &c.props {
            let ty = match d {
                Arg::Bool(_) => "boolean",
                Arg::Text(_) => "string",
            };
            let _ = writeln!(out, "    {p}?: {ty};");
        }
        out.push_str("  }>(),\n  {\n");
        for (p, _, d) in &c.choices {
            let _ = writeln!(out, "    {p}: {},", js_string(d));
        }
        for (p, d) in &c.props {
            let v = match d {
                Arg::Bool(b) => b.to_string(),
                Arg::Text(t) => js_string(t),
            };
            let _ = writeln!(out, "    {p}: {v},");
        }
        out.push_str("  },\n);\n");
    }
    out.push_str("</script>\n\n<template>\n");
    for body in &mut bodies {
        relink_images(body, up);
        body.markup(&mut out, 1, Syntax::Vue);
    }
    out.push_str("</template>\n\n<style scoped>\n");
    for body in &bodies {
        out.push_str(&super::stylesheet(body, up));
    }
    out.push_str("</style>\n");
    out
}

/// A single-quoted JavaScript string, for an expression inside a double-quoted
/// attribute (which [`escape`] then makes safe).
fn single_quoted(s: &str) -> String {
    let inner = js_string(s);
    let inner = &inner[1..inner.len() - 1];
    format!("'{}'", inner.replace("\\\"", "\"").replace('\'', "\\'"))
}

/// Renames every class in a file's trees that is also the root class of a
/// component the file uses.
///
/// A child component's root carries its user's scope too, so a user's rule for
/// one of its own elements would reach a child root of the same class. Rules are
/// printed from the tree, so renaming the element renames its rule.
fn avoid(bodies: &mut [Element], taken: &HashSet<String>) {
    if taken.is_empty() {
        return;
    }
    let mut all = classes_in(bodies);
    all.extend(taken.iter().cloned());
    for body in bodies {
        rename(body, taken, &mut all);
    }
}

fn rename(el: &mut Element, taken: &HashSet<String>, all: &mut HashSet<String>) {
    if el.instance.is_none() && taken.contains(&el.class) {
        let base = el.class.clone();
        let mut n = 2;
        while all.contains(&format!("{base}-{n}")) {
            n += 1;
        }
        el.class = format!("{base}-{n}");
        all.insert(el.class.clone());
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            rename(e, taken, all);
        }
    }
}

/// An `img`'s `src` written from the file, which Vue's compiler turns into an
/// import only when it starts with `.`.
fn relink_images(el: &mut Element, up: &str) {
    if el.tag == "img" {
        for (k, v) in &mut el.attrs {
            if *k == "src" {
                v.insert_str(0, up);
            }
        }
    }
    for c in &mut el.children {
        if let Child::Element(e) = c {
            relink_images(e, up);
        }
    }
}
