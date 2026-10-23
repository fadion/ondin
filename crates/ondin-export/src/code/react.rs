//! The React emitter: TypeScript components, each with a CSS module.
//!
//! `App.tsx` is the page; each main an instance on it uses, or the set it is a
//! variant of, becomes `components/<Name>.tsx` beside `<Name>.module.css` (a
//! kit's in `components/<Kit>/`): its
//! variant properties string unions, its boolean and text properties typed
//! props, each defaulting to the default variant's value. A component's root
//! takes a `className`, which its user places it with. `base.css` holds the page
//! reset every file imports, and `tokens.css` the variables.

use super::dom::{Arg, Syntax, condition};
use super::jsx::{asset_binding, js_string};
use super::walk::{self, Component, Ty};
use super::{BASE_CSS, CodeExport, CodeFile};
use ondin_core::{Document, NodeId, Resolved};
use std::collections::HashMap;
use std::fmt::Write as _;

/// `origins` as `App`, in the order given (later on top).
pub fn react_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, true);
    let tokens = page.tokens.is_some();
    let app = Component::page(page.root.clone());
    let dirs = super::dirs(&page.components);
    let mut files = vec![
        CodeFile {
            path: "App.tsx".into(),
            bytes: module(&app, tokens, &dirs).into_bytes(),
        },
        CodeFile {
            path: "App.module.css".into(),
            bytes: super::stylesheet(&page.root, "./").into_bytes(),
        },
        CodeFile {
            path: "base.css".into(),
            bytes: BASE_CSS.as_bytes().to_vec(),
        },
    ];
    for c in &page.components {
        files.push(CodeFile {
            path: format!("{}/{}.tsx", c.dir, c.name),
            bytes: module(c, tokens, &dirs).into_bytes(),
        });
    }
    for c in &page.components {
        let css: String = c
            .bodies
            .iter()
            .map(|(_, root)| super::stylesheet(root, &super::up(&c.dir)))
            .collect();
        files.push(CodeFile {
            path: format!("{}/{}.module.css", c.dir, c.name),
            bytes: css.into_bytes(),
        });
    }
    super::finish(files, page)
}

/// One `.tsx` file: imports, the props interface for a component, and the
/// function returning its tree — each variant's under the condition it is drawn
/// for. `dirs` holds each component's folder, which its imports are written to.
fn module(c: &Component, tokens: bool, dirs: &HashMap<&str, &str>) -> String {
    let Component {
        name,
        dir,
        bodies,
        props,
    } = c;
    let is_app = name == "App";
    let up = super::up(dir);
    let (uses, images) = super::references(bodies.iter().map(|(_, e)| e));
    let uses = super::with_defaults(uses, props);
    let mut out = String::new();
    if props.iter().any(|(_, ty, _)| *ty == Ty::Component) {
        out.push_str("import type { ComponentType } from \"react\";\n");
    }
    let _ = writeln!(out, "import \"{up}base.css\";");
    if tokens {
        let _ = writeln!(out, "import \"{up}tokens.css\";");
    }
    for u in &uses {
        let at = dirs.get(u.as_str()).copied().unwrap_or("components");
        let from = super::relative(dir, &format!("{at}/{u}"));
        let _ = writeln!(out, "import {u} from \"{from}\";");
    }
    let _ = writeln!(out, "import styles from \"./{name}.module.css\";");
    for img in &images {
        let _ = writeln!(out, "import {} from \"{up}{img}\";", asset_binding(img));
    }
    out.push('\n');

    if is_app {
        let _ = writeln!(out, "export default function App() {{");
    } else {
        let _ = writeln!(out, "export interface {name}Props {{");
        let _ = writeln!(out, "  className?: string;");
        for (p, ty, _) in props {
            let ty = match ty {
                Ty::Bool => "boolean".to_string(),
                Ty::Text => "string".to_string(),
                Ty::Union(values) => {
                    let union: Vec<String> = values.iter().map(|v| js_string(v)).collect();
                    union.join(" | ")
                }
                // What a slot gives any component it draws: the class placing it.
                Ty::Component => "ComponentType<{ className?: string }>".to_string(),
            };
            let _ = writeln!(out, "  {p}?: {ty};");
        }
        out.push_str("}\n\n");
        let mut params = vec!["className".to_string()];
        params.extend(props.iter().map(|(p, ty, d)| {
            // A component is destructured into a capitalised binding, which JSX
            // reads as a tag.
            let bound = match ty {
                Ty::Component => format!("{p}: {}", super::jsx::slot_binding(p)),
                _ => p.clone(),
            };
            match d {
                Some(Arg::Bool(b)) => format!("{bound} = {b}"),
                Some(Arg::Text(t)) => format!("{bound} = {}", js_string(t)),
                Some(Arg::Component(c)) => format!("{bound} = {c}"),
                Some(Arg::Var { .. }) | None => bound,
            }
        }));
        let _ = writeln!(
            out,
            "export default function {name}({{ {} }}: {name}Props) {{",
            params.join(", ")
        );
    }
    for (when, root) in bodies {
        if when.is_empty() {
            out.push_str("  return (\n");
            root.markup(&mut out, 2, Syntax::Jsx);
            out.push_str("  );\n");
        } else {
            let _ = writeln!(out, "  if ({}) {{", condition(when, Syntax::Jsx));
            out.push_str("    return (\n");
            root.markup(&mut out, 3, Syntax::Jsx);
            out.push_str("    );\n  }\n");
        }
    }
    out.push_str("}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{IdSource, NodeKind, Operation, Transaction};

    /// A frame with one rect, as `App` with nothing to import but its styles.
    #[test]
    fn a_page_without_components_is_one_app_module() {
        let mut ids = IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 30.0),
            },
            transform: Some(Affine::IDENTITY),
            name: Some("Card Frame".into()),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let out = react_of(&doc, &res, &[frame]);
        let app = String::from_utf8(out.file("App.tsx").unwrap().to_vec()).unwrap();
        assert!(app.starts_with("import \"./base.css\";\n"), "{app}");
        assert!(
            app.contains("import styles from \"./App.module.css\";"),
            "{app}"
        );
        assert!(app.contains("export default function App() {"), "{app}");
        assert!(
            app.contains("<div className={styles[\"card-frame\"]}>"),
            "{app}"
        );
        let css = String::from_utf8(out.file("App.module.css").unwrap().to_vec()).unwrap();
        assert!(css.contains(".card-frame {"), "{css}");
        assert!(
            !css.contains("box-sizing"),
            "the reset is base.css's: {css}"
        );
        assert!(out.file("base.css").is_some());
    }
}
