//! A layer's **credit**: where an inserted icon or photo came from and what its
//! licence asks for, carried on the node it came with (`Node::credit`).
//!
//! Written once, by the assets browser's insert, and never edited by the user. It
//! rides with every copy (`remap_subtree` clones it), is copied to each child when
//! an icon is ungrouped (`build::ungroup`), and is dropped when the picture it
//! credits is replaced by another file. Never drawn.

use serde::{Deserialize, Serialize};

/// What kind of asset a credit is for: the Credits card groups by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditKind {
    Icon,
    Photo,
}

/// One asset's provenance and attribution.
///
/// Every address is an `http`/`https` link in [`crate::web_link`]'s form, or empty
/// for none: [`Credit::normalized`] holds that on load and the operation refuses
/// anything else, so a reader can open any non-empty one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credit {
    pub kind: CreditKind,
    /// The attribution sentence as the provider asks for it, e.g. *Photo by Ana
    /// Ruiz on Unsplash*. The author's and provider's names appear in it verbatim,
    /// which is how a reader finds the words to link.
    pub text: String,
    /// The photographer or the icon set's author.
    pub author: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub author_url: String,
    /// The service or set it came through: *Unsplash*, *Font Awesome Free*.
    pub provider: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub provider_url: String,
    /// The asset's own page: a photo's. Empty for an icon, so one set's icons
    /// fold into one line (`credits_in`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_url: String,
    /// The licence's name, *CC BY 4.0*, *Unsplash License*.
    pub licence: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub licence_url: String,
    /// Whether the licence asks for the credit to be shown with the design. An
    /// MIT icon keeps its credit as provenance but is not listed in *Credits…*.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// The day it was inserted, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inserted: String,
}

impl Credit {
    /// The addresses a credit holds, in the order a reader lists them.
    fn urls_mut(&mut self) -> [&mut String; 4] {
        [
            &mut self.author_url,
            &mut self.provider_url,
            &mut self.source_url,
            &mut self.licence_url,
        ]
    }

    /// Whether every address is empty or already in [`crate::web_link`]'s form —
    /// what `Operation::SetCredit` asks before writing one.
    pub fn is_well_formed(&self) -> bool {
        [
            &self.author_url,
            &self.provider_url,
            &self.source_url,
            &self.licence_url,
        ]
        .into_iter()
        .all(|u| crate::node::web_link(u).ok().as_deref() == Some(u.as_str()))
    }

    /// This credit with every address not in [`crate::web_link`]'s form emptied —
    /// how a hand-edited file's credit loads, rather than refusing the document.
    pub fn normalized(mut self) -> Self {
        for u in self.urls_mut() {
            if crate::node::web_link(u).ok().as_deref() != Some(u.as_str()) {
                u.clear();
            }
        }
        self
    }

    /// The credit as one plain line: the sentence, then its licence when the
    /// sentence does not name it already. What *Copy credit* and *Copy all* write.
    pub fn plain(&self) -> String {
        if self.licence.is_empty() || self.text.contains(&self.licence) {
            self.text.clone()
        } else {
            format!("{} ({})", self.text, self.licence)
        }
    }
}

/// The operation that drops `id`'s photo credit, when a picture of another file
/// takes the place of the one it credits; `None` when it has none. An icon's
/// credit stays: the picture replaced is not what it credits.
pub fn dropped_by_replace(doc: &crate::Document, id: crate::NodeId) -> Option<crate::Operation> {
    doc.get(id)?
        .credit()
        .filter(|c| c.kind == CreditKind::Photo)
        .map(|_| crate::Operation::SetCredit { id, credit: None })
}

/// One credit as a list shows it: the credit, and the layers carrying it.
#[derive(Clone, Debug, PartialEq)]
pub struct Listed {
    pub credit: Credit,
    pub layers: Vec<crate::NodeId>,
}

/// The credits under `roots`, each with the layers carrying it, in tree order.
/// Layers whose credit says the same thing (an icon set's one sentence) fold into
/// one entry; a layer inside a layer carrying the same credit is not listed
/// again. Hidden layers count:
/// a credit is owed for what the file holds.
pub fn credits_in(doc: &crate::Document, roots: &[crate::NodeId]) -> Vec<Listed> {
    let mut out: Vec<Listed> = Vec::new();
    let mut stack: Vec<(crate::NodeId, Option<&Credit>)> =
        roots.iter().rev().map(|r| (*r, None)).collect();
    while let Some((id, above)) = stack.pop() {
        let Some(node) = doc.get(id) else { continue };
        let own = node.credit();
        if let Some(c) = own.filter(|c| above != Some(*c)) {
            match out.iter_mut().find(|l| same_line(&l.credit, c)) {
                Some(l) => l.layers.push(id),
                None => out.push(Listed {
                    credit: c.clone(),
                    layers: vec![id],
                }),
            }
        }
        let carried = own.or(above);
        stack.extend(node.children().iter().rev().map(|c| (*c, carried)));
    }
    out
}

/// Whether two credits are one line in a list: the same sentence, licence and page.
fn same_line(a: &Credit, b: &Credit) -> bool {
    a.text == b.text && a.licence == b.licence && a.source_url == b.source_url
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo() -> Credit {
        Credit {
            kind: CreditKind::Photo,
            text: "Photo by Ana Ruiz on Unsplash".into(),
            author: "Ana Ruiz".into(),
            author_url: "https://unsplash.com/@ana".into(),
            provider: "Unsplash".into(),
            provider_url: "https://unsplash.com".into(),
            source_url: String::new(),
            licence: "Unsplash License".into(),
            licence_url: String::new(),
            required: true,
            inserted: "2026-11-08".into(),
        }
    }

    /// A non-web address is not well formed and loads emptied; web ones stay.
    #[test]
    fn a_non_web_address_is_refused_and_emptied_on_load() {
        let mut c = photo();
        assert!(c.is_well_formed());
        c.source_url = "javascript:alert(1)".into();
        assert!(!c.is_well_formed());
        let n = c.normalized();
        assert_eq!(n.source_url, "");
        assert_eq!(n.author_url, "https://unsplash.com/@ana");
    }

    /// The plain line names the licence once.
    #[test]
    fn the_plain_line_adds_the_licence_only_when_missing() {
        assert_eq!(
            photo().plain(),
            "Photo by Ana Ruiz on Unsplash (Unsplash License)"
        );
        let mut c = photo();
        c.text = "Icons by Font Awesome, CC BY 4.0".into();
        c.licence = "CC BY 4.0".into();
        assert_eq!(c.plain(), "Icons by Font Awesome, CC BY 4.0");
    }
}
