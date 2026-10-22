//! Code export: a layer tree written as DOM elements and CSS (§7).
//!
//! One core, [`walk`], turns layers into an [`Element`](dom::Element) tree and
//! style rules; an emitter turns that tree into files. Only plain HTML/CSS exists
//! so far ([`html`]). The pipeline is separate from `ExportSpec` -> `plan`: it asks
//! the SVG and PNG writers for what CSS cannot draw rather than rendering it.
//!
//! Each layer goes out in the first of three tiers that draws it exactly: CSS on a
//! `div` (text holding a `p` per paragraph), inline SVG of the subtree, or a PNG
//! of the subtree.
//! [`CodeExport::approximated`] is the contract: whatever is not listed there must
//! look as the canvas does (`tests/code_fidelity.rs` holds the writer to it).
//! [`CodeExport::positioned`] lists what draws right but lost its layout.

mod css;
mod dom;
pub mod html;
mod tokens;
mod walk;

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

/// What the walk learns that is not markup: the two lists of [`CodeExport`].
#[derive(Default)]
pub(crate) struct Report {
    approximated: Vec<String>,
    positioned: Vec<String>,
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

    /// The report as `report.txt` holds it: one line per entry under a heading,
    /// or a single line saying there is nothing to report.
    pub(crate) fn text(&self) -> String {
        if self.approximated.is_empty() && self.positioned.is_empty() {
            return "Everything was written exactly.\n".to_string();
        }
        let mut out = String::new();
        if !self.approximated.is_empty() {
            out.push_str("Drawn differently from the canvas:\n");
            for line in &self.approximated {
                out.push_str("- ");
                out.push_str(line);
                out.push('\n');
            }
        }
        if !self.positioned.is_empty() {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str("Drawn right, with fixed positions or as a picture:\n");
            for line in &self.positioned {
                out.push_str("- ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }
}
