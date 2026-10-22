//! The React emitter: TypeScript components, each with a CSS module.
//!
//! `App.tsx` is the page; each main an instance on it uses becomes
//! `components/<Name>.tsx` beside `<Name>.module.css`, its boolean and text
//! properties typed props defaulting to the main's values. A component's root
//! takes a `className`, which its user places it with. `base.css` holds the page
//! reset every file imports, and `tokens.css` the variables.

use super::dom::{Arg, Element, Syntax};
use super::jsx::{asset_binding, js_string};
use super::walk::{self, Component};
use super::{BASE_CSS, CodeExport, CodeFile};
use ondin_core::{Document, NodeId, Resolved};
use std::fmt::Write as _;

/// `origins` as `App`, in the order given (later on top).
pub fn react_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
    let page = walk::page(doc, res, origins, true);
    let tokens = page.tokens.is_some();
    let mut files = vec![
        CodeFile {
            path: "App.tsx".into(),
            bytes: module("App", &page.root, &[], tokens, "./").into_bytes(),
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
        files.push(component_file(c, tokens));
    }
    for c in &page.components {
        files.push(CodeFile {
            path: format!("components/{}.module.css", c.name),
            bytes: super::stylesheet(&c.root, "../").into_bytes(),
        });
    }
    super::finish(files, page)
}

fn component_file(c: &Component, tokens: bool) -> CodeFile {
    CodeFile {
        path: format!("components/{}.tsx", c.name),
        bytes: module(&c.name, &c.root, &c.props, tokens, "../").into_bytes(),
    }
}

/// One `.tsx` file: imports, the props interface for a component, and the
/// function returning `root`. `up` reaches the export's top folder from the file.
fn module(name: &str, root: &Element, props: &[(String, Arg)], tokens: bool, up: &str) -> String {
    let is_app = name == "App";
    let (uses, images) = super::references(root);
    let mut out = String::new();
    let _ = writeln!(out, "import \"{up}base.css\";");
    if tokens {
        let _ = writeln!(out, "import \"{up}tokens.css\";");
    }
    for u in &uses {
        let from = if is_app {
            format!("./components/{u}")
        } else {
            format!("./{u}")
        };
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
        for (p, d) in props {
            let ty = match d {
                Arg::Bool(_) => "boolean",
                Arg::Text(_) => "string",
            };
            let _ = writeln!(out, "  {p}?: {ty};");
        }
        out.push_str("}\n\n");
        let mut params = vec!["className".to_string()];
        params.extend(props.iter().map(|(p, d)| match d {
            Arg::Bool(b) => format!("{p} = {b}"),
            Arg::Text(t) => format!("{p} = {}", js_string(t)),
        }));
        let _ = writeln!(
            out,
            "export default function {name}({{ {} }}: {name}Props) {{",
            params.join(", ")
        );
    }
    out.push_str("  return (\n");
    root.markup(&mut out, 2, Syntax::Jsx);
    out.push_str("  );\n}\n");
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
