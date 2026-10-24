//! Code export: a layer tree written as DOM elements and CSS (§7).
//!
//! One core, [`walk`], turns layers into an [`Element`](dom::Element) tree and
//! style rules; an emitter turns that tree into files: plain HTML/CSS ([`html`]),
//! React ([`react`]) or Vue ([`vue`]). The two frameworks write an instance as a
//! use of a component made from its main. The pipeline is separate from
//! `ExportSpec` -> `plan`: it asks the SVG and PNG writers for what CSS cannot
//! draw rather than rendering it.
//!
//! Each layer goes out in the first of three tiers that draws it exactly: CSS on a
//! `div` (text holding a `p` per paragraph), inline SVG of the subtree, or a PNG
//! of the subtree.
//! [`CodeExport::approximated`] is the contract: whatever is not listed there must
//! look as the canvas does (`tests/code_fidelity.rs` holds the writer to it).
//! [`CodeExport::positioned`] lists what draws right but lost its layout.

use ondin_core::{Document, NodeId, Resolved};

mod css;
mod dom;
pub mod html;
mod image;
mod jsx;
mod merge;
pub mod react;
mod tokens;
pub mod vue;
mod walk;

/// What a code export is written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Html,
    React,
    Vue,
}

impl Target {
    pub const ALL: [Target; 3] = [Target::Html, Target::React, Target::Vue];

    /// The name a person knows it by.
    pub fn label(self) -> &'static str {
        match self {
            Target::Html => "HTML",
            Target::React => "React",
            Target::Vue => "Vue",
        }
    }

    /// `origins` written as this target, in the order given (later on top).
    pub fn export(self, doc: &Document, res: &Resolved, origins: &[NodeId]) -> CodeExport {
        match self {
            Target::Html => html::html_of(doc, res, origins),
            Target::React => react::react_of(doc, res, origins),
            Target::Vue => vue::vue_of(doc, res, origins),
        }
    }
}

/// *Copy as CSS*: each of `ids`' own declarations, under a comment naming the
/// layer — what the HTML export writes on its class, without its children's rules.
///
/// Placement (`position`, `left`, `top`) is left out: each layer is written as
/// its own page, so it would always read as the origin. A variable stays a
/// `var(--…)`.
pub fn css_of(doc: &Document, res: &Resolved, ids: &[NodeId]) -> String {
    let mut out = String::new();
    for id in ids {
        let Some(node) = doc.get(*id) else { continue };
        let page = walk::page(doc, res, &[*id], false);
        let Some(dom::Child::Element(el)) = page.root.children.first() else {
            continue;
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("/* {} */\n", node.name().replace("*/", "* /")));
        for (prop, value) in &el.style {
            if !matches!(*prop, "position" | "left" | "top") {
                out.push_str(&format!("{prop}: {};\n", lone_solid(value)));
            }
        }
    }
    out
}

/// A background of one solid layer as its colour: the walk writes a solid as a
/// one-colour gradient so it can stack, which a pasted rule does not need. Split
/// at the middle, not the first comma: a colour can hold one (`color-mix(…)`).
fn lone_solid(value: &str) -> &str {
    let Some(inner) = value
        .strip_prefix("linear-gradient(")
        .and_then(|v| v.strip_suffix(')'))
    else {
        return value;
    };
    let half = inner.len().saturating_sub(2) / 2;
    match (
        inner.get(..half),
        inner.get(half..half + 2),
        inner.get(half + 2..),
    ) {
        (Some(a), Some(", "), Some(b)) if a == b && !a.is_empty() => a,
        _ => value,
    }
}

/// One file of a code export, at its path inside the export's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFile {
    pub path: String,
    pub bytes: Vec<u8>,
}

/// A code export and what it could not say exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeExport {
    /// Every file, `report.txt` included, in the order a zip lists them.
    pub files: Vec<CodeFile>,
    /// Where the page draws differently from the canvas. Deduplicated, in order met.
    pub approximated: Vec<String>,
    /// Where the page draws right but a layer's layout became fixed positions or
    /// a picture, so it will not respond as the design does.
    pub positioned: Vec<String>,
    /// Instances written out as their layers rather than as a use of their
    /// main's component, and why. Empty for plain HTML, which has no components.
    pub inlined: Vec<String>,
}

impl CodeExport {
    /// The file at `path`, if the export wrote one.
    pub fn file(&self, path: &str) -> Option<&[u8]> {
        self.files
            .iter()
            .find(|f| f.path == path)
            .map(|f| f.bytes.as_slice())
    }

    /// Every file in one zip, stored under its own path ([`crate::plan::zip`]).
    pub fn zip(&self) -> std::io::Result<Vec<u8>> {
        let files: Vec<(String, Vec<u8>)> = self
            .files
            .iter()
            .map(|f| (f.path.clone(), f.bytes.clone()))
            .collect();
        crate::plan::zip(&files)
    }
}

/// What the walk learns that is not markup: the three lists of [`CodeExport`].
#[derive(Default)]
pub(crate) struct Report {
    approximated: Vec<String>,
    positioned: Vec<String>,
    inlined: Vec<String>,
}

impl Report {
    /// The page draws this differently. Deduplicated by wording.
    pub(crate) fn approximate(&mut self, what: impl Into<String>) {
        let what = what.into();
        if !self.approximated.contains(&what) {
            self.approximated.push(what);
        }
    }

    /// The page draws this right, as fixed positions or a picture.
    pub(crate) fn position(&mut self, what: impl Into<String>) {
        let what = what.into();
        if !self.positioned.contains(&what) {
            self.positioned.push(what);
        }
    }

    /// An instance is written out as its layers.
    pub(crate) fn inline(&mut self, what: impl Into<String>) {
        let what = what.into();
        if !self.inlined.contains(&what) {
            self.inlined.push(what);
        }
    }

    /// The report as `report.txt` holds it: one line per entry under a heading,
    /// or a single line saying there is nothing to report.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        for (heading, lines) in [
            ("Drawn differently from the canvas:", &self.approximated),
            (
                "Drawn right, with fixed positions or as a picture:",
                &self.positioned,
            ),
            ("Written out as layers, not as a component:", &self.inlined),
        ] {
            if lines.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(heading);
            out.push('\n');
            for line in lines {
                out.push_str("- ");
                out.push_str(line);
                out.push('\n');
            }
        }
        if out.is_empty() {
            out.push_str("Everything was written exactly.\n");
        }
        out
    }

    /// The report's lists, for [`CodeExport`].
    pub(crate) fn into_lists(self) -> (Vec<String>, Vec<String>, Vec<String>) {
        (self.approximated, self.positioned, self.inlined)
    }
}

/// `tokens.css`, the pictures and `report.txt`, which every emitter writes the
/// same way, after its own files.
pub(crate) fn finish(mut files: Vec<CodeFile>, page: walk::Page) -> CodeExport {
    if let Some(tokens) = page.tokens {
        files.push(CodeFile {
            path: "tokens.css".into(),
            bytes: tokens.into_bytes(),
        });
    }
    files.extend(
        page.assets
            .into_iter()
            .map(|(path, bytes)| CodeFile { path, bytes }),
    );
    files.push(CodeFile {
        path: "report.txt".into(),
        bytes: page.report.text().into_bytes(),
    });
    let (approximated, positioned, inlined) = page.report.into_lists();
    CodeExport {
        files,
        approximated,
        positioned,
        inlined,
    }
}

/// The rules of `root`'s subtree for a stylesheet whose way to the export's top
/// folder is `up` (`./`, `../`), its pictures' URLs written from there, as a
/// bundler resolves them.
pub(crate) fn stylesheet(root: &dom::Element, up: &str) -> String {
    let mut css = String::new();
    root.css(&mut css);
    css.replace("url(\"assets/", &format!("url(\"{up}assets/"))
}

/// Each component's folder, by name.
pub(crate) fn dirs(components: &[walk::Component]) -> std::collections::HashMap<&str, &str> {
    components
        .iter()
        .map(|c| (c.name.as_str(), c.dir.as_str()))
        .collect()
}

/// The way back to the export's top folder from a file in `dir`: `./`, `../`,
/// `../../`.
pub(crate) fn up(dir: &str) -> String {
    match dir.split('/').filter(|s| !s.is_empty()).count() {
        0 => "./".into(),
        n => "../".repeat(n),
    }
}

/// `path`, from the export's top folder, as an import in a file in `dir`
/// writes it.
pub(crate) fn relative(dir: &str, path: &str) -> String {
    let from: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = path.split('/').collect();
    // The last of `to` is the file, never a folder `from` shares.
    let shared = from
        .iter()
        .zip(&to[..to.len() - 1])
        .take_while(|(a, b)| a == b)
        .count();
    up(&from[shared..].join("/")) + &to[shared..].join("/")
}

/// The components the trees under `roots` use and the pictures they link, each
/// once, in document order. A slot's default and a component passed to a swap
/// property are uses: the file names both.
pub(crate) fn references<'a>(
    roots: impl IntoIterator<Item = &'a dom::Element>,
) -> (Vec<String>, Vec<String>) {
    let mut uses = Vec::new();
    let mut images = Vec::new();
    roots.into_iter().for_each(|root| {
        root.each(&mut |e| {
            if let Some(i) = &e.instance {
                let mut named = vec![&i.component];
                for (_, a) in &i.args {
                    let mut a = Some(a);
                    while let Some(arg) = a {
                        a = match arg {
                            dom::Arg::Component(c) => {
                                named.push(c);
                                None
                            }
                            dom::Arg::Var { fallback, .. } => fallback.as_deref(),
                            _ => None,
                        };
                    }
                }
                for c in named {
                    if !uses.contains(c) {
                        uses.push(c.clone());
                    }
                }
            }
            if e.tag == "img"
                && let Some((_, src)) = e.attrs.iter().find(|(k, _)| *k == "src")
                && !images.contains(src)
            {
                images.push(src.clone());
            }
            // An inline SVG's linked pictures.
            for link in e.markup.iter().flat_map(|m| m.split("href=\"").skip(1)) {
                if let Some((path, _)) = link.split_once('"')
                    && path.starts_with("assets/")
                    && !images.iter().any(|i| i == path)
                {
                    images.push(path.to_string());
                }
            }
        })
    });
    (uses, images)
}

/// `uses` with every component a swap property defaults to, which the file
/// names whether or not a slot of it is drawn.
pub(crate) fn with_defaults(
    mut uses: Vec<String>,
    props: &[(String, walk::Ty, Option<dom::Arg>)],
) -> Vec<String> {
    for (_, _, d) in props {
        if let Some(dom::Arg::Component(c)) = d
            && !uses.contains(c)
        {
            uses.push(c.clone());
        }
    }
    uses
}

/// Rules every page starts with: no default margins, and sizes that include
/// padding and borders, which is how the model measures a box.
pub(crate) const BASE_CSS: &str = "\
*, *::before, *::after {
  box-sizing: border-box;
}
body {
  margin: 0;
}
";

#[cfg(test)]
mod tests {
    use super::*;

    /// An import between the page, `components` and a kit's folder under it
    /// climbs only out of the folders the two files do not share.
    #[test]
    fn imports_climb_out_of_the_folders_not_shared() {
        assert_eq!(up(""), "./");
        assert_eq!(up("components/Kit"), "../../");
        for (dir, path, import) in [
            ("", "components/Card", "./components/Card"),
            ("", "components/Kit/Badge", "./components/Kit/Badge"),
            ("components", "components/Card", "./Card"),
            ("components", "components/Kit/Badge", "./Kit/Badge"),
            ("components/Kit", "components/Kit/Dot", "./Dot"),
            ("components/Kit", "components/Card", "../Card"),
            ("components/Kit", "components/Other/Dot", "../Other/Dot"),
            ("components/Kit", "assets/a.png", "../../assets/a.png"),
        ] {
            assert_eq!(relative(dir, path), import, "{dir} -> {path}");
        }
    }

    /// *Copy as CSS* names the layer and writes its own box and paint, without
    /// the placement every one-layer page puts at the origin.
    #[test]
    fn css_of_writes_the_layers_own_declarations() {
        use ondin_core::kurbo::{Affine, Size};
        use ondin_core::peniko::Color;
        use ondin_core::{Brush, Fill, IdSource, NodeKind, Operation, Transaction};
        let mut ids = IdSource::new(0x5C);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: rect,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(120.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((300.0, 200.0))),
                name: Some("Card */ x".into()),
            },
            Operation::SetFills {
                id: rect,
                fills: ondin_core::keyed_by_position([Fill {
                    brush: Brush::Solid(Color::from_rgba8(255, 0, 0, 255)).into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let css = css_of(&doc, &res, &[rect]);
        assert!(css.starts_with("/* Card * / x */\n"), "{css}");
        assert!(css.contains("width: 120px;"), "{css}");
        assert!(css.contains("height: 40px;"), "{css}");
        assert!(css.contains("background: #ff0000;"), "{css}");
        assert!(
            !css.contains("position:") && !css.contains("left:"),
            "{css}"
        );
    }

    /// The collapse takes a colour holding commas whole and leaves a real
    /// two-colour ramp alone.
    ///
    /// flip: `lone_solid` splitting at the first `", "` → fails at the
    /// `color-mix` case.
    #[test]
    fn a_lone_solid_collapses_whatever_its_colour_holds() {
        let mix = "color-mix(in srgb, var(--c) 50%, transparent)";
        assert_eq!(lone_solid(&format!("linear-gradient({mix}, {mix})")), mix);
        assert_eq!(lone_solid("linear-gradient(#fff, #fff)"), "#fff");
        let ramp = "linear-gradient(#fff, #000)";
        assert_eq!(lone_solid(ramp), ramp);
        let two = "linear-gradient(#fff, #fff), linear-gradient(#000, #000)";
        assert_eq!(lone_solid(two), two, "two stacked layers stay as written");
    }
}
