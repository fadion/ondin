//! The HTML element a code export writes a layer as, and the address a link
//! takes.
//!
//! A layer with no element is *Automatic*: the export picks `div`, `p`/`span`,
//! `img` or `svg` from what the layer is. A frame or a group may be a landmark,
//! a list, a button or a link; a text a heading, a button or a link; anything
//! else is written as a picture or a plain box and takes none. The rules that
//! keep the HTML valid ([`refusal`]) are the menu's; a later edit (a drag into a
//! button) can still break one, and the export writes such a layer as Automatic
//! and says so.

use crate::document::Document;
use crate::id::NodeId;
use crate::node::{Node, NodeKind};
use crate::op::Operation;
use serde::{Deserialize, Serialize};

/// An element a layer can be written as, saved under its tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HtmlElement {
    #[serde(rename = "main")]
    Main,
    #[serde(rename = "section")]
    Section,
    #[serde(rename = "article")]
    Article,
    #[serde(rename = "aside")]
    Aside,
    #[serde(rename = "header")]
    Header,
    #[serde(rename = "footer")]
    Footer,
    #[serde(rename = "nav")]
    Nav,
    /// A `ul`, whose children are its items.
    #[serde(rename = "ul")]
    List,
    #[serde(rename = "button")]
    Button,
    #[serde(rename = "a")]
    Link,
    #[serde(rename = "h1")]
    H1,
    #[serde(rename = "h2")]
    H2,
    #[serde(rename = "h3")]
    H3,
    #[serde(rename = "h4")]
    H4,
    #[serde(rename = "h5")]
    H5,
    #[serde(rename = "h6")]
    H6,
}

/// What a frame or a group may be, in the menu's order.
pub const CONTAINER: [HtmlElement; 10] = [
    HtmlElement::Main,
    HtmlElement::Section,
    HtmlElement::Article,
    HtmlElement::Aside,
    HtmlElement::Header,
    HtmlElement::Footer,
    HtmlElement::Nav,
    HtmlElement::List,
    HtmlElement::Button,
    HtmlElement::Link,
];

/// What a text may be, in the menu's order.
pub const TEXT: [HtmlElement; 8] = [
    HtmlElement::H1,
    HtmlElement::H2,
    HtmlElement::H3,
    HtmlElement::H4,
    HtmlElement::H5,
    HtmlElement::H6,
    HtmlElement::Button,
    HtmlElement::Link,
];

impl HtmlElement {
    /// The tag it is written as.
    pub fn tag(self) -> &'static str {
        match self {
            HtmlElement::Main => "main",
            HtmlElement::Section => "section",
            HtmlElement::Article => "article",
            HtmlElement::Aside => "aside",
            HtmlElement::Header => "header",
            HtmlElement::Footer => "footer",
            HtmlElement::Nav => "nav",
            HtmlElement::List => "ul",
            HtmlElement::Button => "button",
            HtmlElement::Link => "a",
            HtmlElement::H1 => "h1",
            HtmlElement::H2 => "h2",
            HtmlElement::H3 => "h3",
            HtmlElement::H4 => "h4",
            HtmlElement::H5 => "h5",
            HtmlElement::H6 => "h6",
        }
    }

    /// The app's word for it.
    pub fn label(self) -> &'static str {
        match self {
            HtmlElement::Main => "Main",
            HtmlElement::Section => "Section",
            HtmlElement::Article => "Article",
            HtmlElement::Aside => "Aside",
            HtmlElement::Header => "Header",
            HtmlElement::Footer => "Footer",
            HtmlElement::Nav => "Navigation",
            HtmlElement::List => "List",
            HtmlElement::Button => "Button",
            HtmlElement::Link => "Link",
            HtmlElement::H1 => "Heading 1",
            HtmlElement::H2 => "Heading 2",
            HtmlElement::H3 => "Heading 3",
            HtmlElement::H4 => "Heading 4",
            HtmlElement::H5 => "Heading 5",
            HtmlElement::H6 => "Heading 6",
        }
    }

    /// A button or a link: interactive content, which cannot nest.
    pub fn interactive(self) -> bool {
        matches!(self, HtmlElement::Button | HtmlElement::Link)
    }

    pub fn heading(self) -> bool {
        matches!(
            self,
            HtmlElement::H1
                | HtmlElement::H2
                | HtmlElement::H3
                | HtmlElement::H4
                | HtmlElement::H5
                | HtmlElement::H6
        )
    }

    /// A sectioning element or landmark that may not hold the page's main.
    fn bars_main(self) -> bool {
        matches!(
            self,
            HtmlElement::Article
                | HtmlElement::Aside
                | HtmlElement::Header
                | HtmlElement::Footer
                | HtmlElement::Nav
        )
    }

    fn header_or_footer(self) -> bool {
        matches!(self, HtmlElement::Header | HtmlElement::Footer)
    }

    /// Its name in a sentence: "a button", "the main".
    fn noun(self) -> String {
        match self {
            HtmlElement::Main => "the main".into(),
            HtmlElement::Nav => "a navigation".into(),
            HtmlElement::Aside | HtmlElement::Article => {
                format!("an {}", self.label().to_lowercase())
            }
            e if e.heading() => "a heading".into(),
            e => format!("a {}", e.label().to_lowercase()),
        }
    }
}

/// The elements `kind` may be written as, or `None` when it takes none.
pub fn offered(kind: &NodeKind) -> Option<&'static [HtmlElement]> {
    match kind {
        NodeKind::Artboard { .. } | NodeKind::Group => Some(&CONTAINER),
        NodeKind::Text { .. } => Some(&TEXT),
        _ => None,
    }
}

/// Why a layer of `kind` takes no element, or `None` when it takes one. A
/// slice is never written to code and shows no row at all.
pub fn kind_refusal(kind: &NodeKind) -> Option<&'static str> {
    match offered(kind) {
        Some(_) => None,
        None => Some(
            "A shape is written as a picture (svg or img) or a plain box — \
             put it in a frame to give it an element",
        ),
    }
}

/// What the export writes a layer of `kind` as on Automatic, for the menu's
/// faint tag: `div` for a frame or group, `p` for a text, `li` in a list.
pub fn automatic_tag(doc: &Document, node: &Node) -> &'static str {
    if in_list(doc, node.id()) {
        return "li";
    }
    match node.kind() {
        NodeKind::Text { .. } => "p",
        NodeKind::Artboard { .. } | NodeKind::Group => "div",
        _ => "svg",
    }
}

/// Whether `id`'s parent is a list, which makes it a list item.
pub fn in_list(doc: &Document, id: NodeId) -> bool {
    doc.get(id)
        .and_then(Node::parent)
        .and_then(|p| doc.get(p))
        .is_some_and(|p| p.element() == Some(HtmlElement::List))
}

/// The layer that is its own top-level frame, or the frame directly on a page
/// above `id`: the one document its HTML is written into.
fn top_of(doc: &Document, id: NodeId) -> NodeId {
    let mut at = id;
    while let Some(p) = doc.get(at).and_then(Node::parent) {
        if doc.is_page(p) || doc.get(p).and_then(Node::parent).is_none() {
            break;
        }
        at = p;
    }
    at
}

/// Every layer below `id`, not `id` itself, in Layers order: the topmost
/// sibling first, each above what it holds.
fn below(doc: &Document, id: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = doc
        .get(id)
        .map(|n| n.children().to_vec())
        .unwrap_or_default();
    while let Some(at) = stack.pop() {
        out.push(at);
        if let Some(n) = doc.get(at) {
            stack.extend(n.children().iter().copied());
        }
    }
    out
}

/// How the layer `id` is named in a reason.
fn quoted(doc: &Document, id: NodeId) -> String {
    let name = doc.get(id).map(Node::name).unwrap_or_default();
    match name.is_empty() {
        true => "a layer".into(),
        false => format!("“{name}”"),
    }
}

/// The paragraphs of a text, or 1 for any other layer.
fn paragraphs(kind: &NodeKind) -> usize {
    match kind {
        NodeKind::Text { content, .. } => content.split('\n').count(),
        _ => 1,
    }
}

/// Why `id` cannot be `element`, or `None` when it can: the reasons the menu
/// shows on a disabled row, naming the nearest ancestor that causes one, else
/// the first such layer in Layers order. `element` is one [`offered`] for the
/// layer's kind.
pub fn refusal(doc: &Document, id: NodeId, element: HtmlElement) -> Option<String> {
    let node = doc.get(id)?;
    let ancestors: Vec<(NodeId, HtmlElement)> = {
        let mut out = Vec::new();
        let mut at = node.parent();
        while let Some(p) = at.and_then(|p| doc.get(p)) {
            if let Some(e) = p.element() {
                out.push((p.id(), e));
            }
            at = p.parent();
        }
        out
    };
    let inside: Vec<(NodeId, HtmlElement)> = below(doc, id)
        .into_iter()
        .filter_map(|d| Some((d, doc.get(d)?.element()?)))
        .collect();
    // Inside a button only phrasing content is allowed: every layer is a span.
    if let Some((b, _)) = ancestors.iter().find(|(_, e)| *e == HtmlElement::Button) {
        return Some(format!(
            "Inside a button a layer is written as a span — {} is a button",
            quoted(doc, *b)
        ));
    }
    if element.interactive() {
        if let Some((a, e)) = ancestors.iter().find(|(_, e)| e.interactive()) {
            return Some(format!(
                "{} can't sit inside {} — {} is one",
                capital(&element.noun()),
                e.noun(),
                quoted(doc, *a)
            ));
        }
        if let Some((d, e)) = inside.iter().find(|(_, e)| e.interactive()) {
            return Some(format!(
                "{} can't hold {} — {}, inside it, is one",
                capital(&element.noun()),
                e.noun(),
                quoted(doc, *d)
            ));
        }
    }
    if element == HtmlElement::Button && !inside.is_empty() {
        let (d, _) = inside[0];
        return Some(format!(
            "Inside a button a layer is written as a span — {} has an element",
            quoted(doc, d)
        ));
    }
    let paras = paragraphs(node.kind());
    if paras > 1 && (element.heading() || element == HtmlElement::Button) {
        let what = match element {
            HtmlElement::Button => "A button",
            _ => "A heading",
        };
        return Some(format!("{what} is one paragraph — this text has {paras}"));
    }
    if element == HtmlElement::Main {
        let top = top_of(doc, id);
        let others = std::iter::once(top).chain(below(doc, top));
        if let Some(m) = others
            .filter(|o| *o != id)
            .find(|o| doc.get(*o).and_then(Node::element) == Some(HtmlElement::Main))
        {
            return Some(format!("A page has one main — {} is it", quoted(doc, m)));
        }
        if let Some((a, e)) = ancestors.iter().find(|(_, e)| e.bars_main()) {
            return Some(format!(
                "The main can't sit inside {} — {} is one",
                e.noun(),
                quoted(doc, *a)
            ));
        }
    }
    if element.bars_main()
        && let Some((d, _)) = inside.iter().find(|(_, e)| *e == HtmlElement::Main)
    {
        return Some(format!(
            "{} can't hold the main — {}, inside it, is it",
            capital(&element.noun()),
            quoted(doc, *d)
        ));
    }
    if element.header_or_footer() {
        if let Some((a, e)) = ancestors.iter().find(|(_, e)| e.header_or_footer()) {
            return Some(format!(
                "{} can't sit inside {} — {} is one",
                capital(&element.noun()),
                e.noun(),
                quoted(doc, *a)
            ));
        }
        if let Some((d, e)) = inside.iter().find(|(_, e)| e.header_or_footer()) {
            return Some(format!(
                "{} can't hold {} — {}, inside it, is one",
                capital(&element.noun()),
                e.noun(),
                quoted(doc, *d)
            ));
        }
    }
    None
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The layers one choice of element writes: a variant root writes every root of
/// its set, since a component has one element per root; any other layer only
/// itself.
pub fn targets(doc: &Document, id: NodeId) -> Vec<NodeId> {
    let Some(node) = doc.get(id) else {
        return Vec::new();
    };
    if node.component()
        && let Some(set) = crate::variant::set_of(doc, id)
    {
        return crate::variant::variants(doc, set);
    }
    if node.set().is_some() {
        return crate::variant::variants(doc, id);
    }
    vec![id]
}

/// The ops writing `element` onto `id` (every root of its set, for a variant),
/// leaving the layers already holding it alone. Empty when nothing changes.
pub fn set_ops(doc: &Document, id: NodeId, element: Option<HtmlElement>) -> Vec<Operation> {
    targets(doc, id)
        .into_iter()
        .filter(|t| doc.get(*t).is_some_and(|n| n.element() != element))
        .map(|t| Operation::SetElement { id: t, element })
        .collect()
}

/// Whether `typed` is an address a link can take: `https:`, `http:`,
/// `mailto:` or `tel:`, a `#` fragment, or a relative path. Empty is none,
/// which writes a link with no `href`, a valid placeholder.
pub fn is_href(typed: &str) -> bool {
    if typed.is_empty() {
        return true;
    }
    if typed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let lower = typed.to_ascii_lowercase();
    for scheme in ["https://", "http://", "mailto:", "tel:"] {
        if let Some(rest) = lower.strip_prefix(scheme) {
            return !rest.is_empty();
        }
    }
    // Any other scheme (`javascript:`, `data:`) is refused; a relative path
    // has no colon before its first slash, query or fragment.
    let head = typed.split(['/', '?', '#']).next().unwrap_or_default();
    !head.contains(':') && !typed.starts_with("//")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address forms a link takes, and the ones it refuses.
    #[test]
    fn addresses_a_link_takes() {
        for ok in [
            "",
            "https://ondin.app",
            "HTTP://x.y/z",
            "mailto:a@b.c",
            "tel:+1555",
            "#top",
            "/signup",
            "docs/intro.html",
            "?q=1",
        ] {
            assert!(is_href(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "https://",
            "two words",
            "//cdn.example",
            "ftp://x",
        ] {
            assert!(!is_href(bad), "{bad}");
        }
    }

    /// Every tag is the serde name, so the file holds what the HTML says.
    #[test]
    fn the_saved_name_is_the_tag() {
        for e in CONTAINER.iter().chain(&TEXT) {
            let saved = serde_json::to_string(e).unwrap();
            assert_eq!(saved, format!("\"{}\"", e.tag()));
        }
    }
}
