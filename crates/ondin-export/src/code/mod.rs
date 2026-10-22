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

/// The components `root`'s subtree uses and the pictures it links, each once,
/// in document order.
pub(crate) fn references(root: &dom::Element) -> (Vec<String>, Vec<String>) {
    let mut uses = Vec::new();
    let mut images = Vec::new();
    root.each(&mut |e| {
        if let Some(i) = &e.instance
            && !uses.contains(&i.component)
        {
            uses.push(i.component.clone());
        }
        if e.tag == "img"
            && let Some((_, src)) = e.attrs.iter().find(|(k, _)| *k == "src")
            && !images.contains(src)
        {
            images.push(src.clone());
        }
    });
    (uses, images)
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
