//! The Vue emitter: single-file components in TypeScript with scoped styles.
//!
//! `App.vue` is the page; each main an instance on it uses becomes
//! `components/<Name>.vue`, its boolean and text properties typed props
//! defaulting to the main's values. A class on a component falls through to its
//! root, and the user's scoped rule reaches that root, so the user places it.
//! `base.css` holds the page reset every file imports, and `tokens.css` the
//! variables.

use super::dom::{Arg, Child, Element, Syntax};
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
    let roots: HashMap<&str, &str> = page
        .components
        .iter()
        .map(|c| (c.name.as_str(), c.root.class.as_str()))
        .collect();
    let mut files = vec![
        CodeFile {
            path: "App.vue".into(),
            bytes: sfc("App", &page.root, &[], tokens, "./", &roots).into_bytes(),
        },
        CodeFile {
            path: "base.css".into(),
            bytes: BASE_CSS.as_bytes().to_vec(),
        },
    ];
    for Component { name, root, props } in &page.components {
        files.push(CodeFile {
            path: format!("components/{name}.vue"),
            bytes: sfc(name, root, props, tokens, "../", &roots).into_bytes(),
        });
    }
    super::finish(files, page)
}

/// One `.vue` file. `up` reaches the export's top folder from the file; `roots`
/// is each component's root class.
fn sfc(
    name: &str,
    root: &Element,
    props: &[(String, Arg)],
    tokens: bool,
    up: &str,
    roots: &HashMap<&str, &str>,
) -> String {
    let is_app = name == "App";
    let (uses, _) = super::references(root);
    let mut root = root.clone();
    let taken: HashSet<String> = uses
        .iter()
        .filter_map(|u| roots.get(u.as_str()))
        .map(|c| c.to_string())
        .collect();
    avoid(&mut root, &taken);
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
    if !props.is_empty() {
        out.push_str("\nwithDefaults(\n  defineProps<{\n");
        for (p, d) in props {
            let ty = match d {
                Arg::Bool(_) => "boolean",
                Arg::Text(_) => "string",
            };
            let _ = writeln!(out, "    {p}?: {ty};");
        }
        out.push_str("  }>(),\n  {\n");
        for (p, d) in props {
            let v = match d {
                Arg::Bool(b) => b.to_string(),
                Arg::Text(t) => js_string(t),
            };
            let _ = writeln!(out, "    {p}: {v},");
        }
        out.push_str("  },\n);\n");
    }
    out.push_str("</script>\n\n<template>\n");
    relink_images(&mut root, up);
    root.markup(&mut out, 1, Syntax::Vue);
    out.push_str("</template>\n\n<style scoped>\n");
    out.push_str(&super::stylesheet(&root, up));
    out.push_str("</style>\n");
    out
}

/// Renames every class in `root`'s file that is also the root class of a
/// component the file uses.
///
/// A child component's root carries its user's scope too, so a user's rule for
/// one of its own elements would reach a child root of the same class. Rules are
/// printed from the tree, so renaming the element renames its rule.
fn avoid(root: &mut Element, taken: &HashSet<String>) {
    if taken.is_empty() {
        return;
    }
    let mut all = HashSet::new();
    root.each(&mut |e| {
        all.insert(e.class.clone());
    });
    all.extend(taken.iter().cloned());
    rename(root, taken, &mut all);
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
