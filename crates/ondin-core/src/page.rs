//! Pages: a document's separate canvases (§5.5).
//!
//! A page is a parentless `Root` node listed in [`Document::pages`]. Its layers
//! are that root's subtree; its name is the root's name and its default mode the
//! root's [`Node::mode`]. What is not a node field lives in [`Page`]: the ground
//! and the draft flag.
//!
//! Everything else — components, styles, variables, images, the kit vault —
//! belongs to the document and is shared by every page. Every page has the same
//! origin, so a layer moved between pages keeps its coordinates.
//!
//! [`Document::pages`]: crate::document::Document::pages
//! [`Node::mode`]: crate::node::Node

use crate::id::NodeId;
use peniko::Color;

/// The name a new document's one page carries, and the stem new pages number
/// from (`Page 2`, `Page 3`, …).
pub const PAGE_STEM: &str = "Page";

/// The page-table half of a page: what is not a field of its root node.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    /// The page's root node.
    pub id: NodeId,
    /// The colour behind and around the page's frames. Opaque by construction:
    /// nothing behind the canvas could show through.
    pub ground: Color,
    /// Left out of everything that gathers from every page — kit publishing,
    /// exporting all pages, presenting on past the current page, the library
    /// thumbnail — and kept by everything aimed at it directly.
    pub draft: bool,
}

/// `Page N` for the lowest `N ≥ 1` no name in `taken` already uses.
pub fn free_name<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: Vec<&str> = taken.into_iter().collect();
    (1..)
        .map(|n| format!("{PAGE_STEM} {n}"))
        .find(|name| !taken.contains(&name.as_str()))
        .expect("an unbounded range finds a free number")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `free_name` fills the lowest gap rather than counting past the highest.
    #[test]
    fn free_name_takes_the_lowest_unused_number() {
        assert_eq!(free_name([]), "Page 1");
        assert_eq!(free_name(["Page 1", "Page 3"]), "Page 2");
        assert_eq!(free_name(["Landing", "Page 1", "Page 2"]), "Page 3");
    }
}
