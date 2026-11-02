//! Find and replace over what text layers say: every match of a query in the
//! stored content of the text layers in a scope, and the edit that replaces some
//! of them. Layer names are batch rename's (`crate::rename`), whose matcher this
//! shares.
//!
//! Matching reads the **stored** content: a case transform is drawn, not stored,
//! and a list marker is drawn, not text, so neither is ever matched. A text whose
//! content is bound to a variable stores a placeholder and is skipped.

use crate::rename::{Find, Piece, Rule, Template};
use crate::{Document, NodeId, NodeKind, Operation, Transaction};
use std::collections::HashMap;
use std::ops::Range;

/// What a query looks for and what it writes in place of a match.
#[derive(Clone, Debug)]
pub struct Query(Rule);

impl Query {
    /// `find` as typed, matched ignoring case unless `match_case`, only between
    /// word boundaries with `whole_words`, and as a regular expression with
    /// `pattern` — when a pattern's `[1]` in `replace` inserts its first group.
    /// `keep_case` gives each replacement the capitals of what it replaces. A
    /// pattern that does not parse is `Err` with a one-line reason.
    pub fn new(
        find: &str,
        replace: &str,
        match_case: bool,
        whole_words: bool,
        pattern: bool,
        keep_case: bool,
    ) -> Result<Self, String> {
        let template = if pattern {
            Template::parse(replace)
        } else {
            Template(vec![Piece::Text(replace.to_string())])
        };
        Ok(Self(Rule {
            find: Find::with_words(find, match_case, pattern, whole_words)?,
            template,
            keep_case,
        }))
    }

    /// Whether there is nothing to look for.
    pub fn is_empty(&self) -> bool {
        self.0.find.is_whole()
    }

    /// What replaces the match at `range` of `content`, if it is still one.
    pub fn replacement(&self, content: &str, range: &Range<usize>) -> Option<String> {
        self.0
            .replacements(content, "")
            .into_iter()
            .find(|(r, _)| r == range)
            .map(|(_, with)| with)
    }
}

/// Where a query looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// These layers and everything under them.
    Under(Vec<NodeId>),
    /// One page.
    Page(NodeId),
    /// Every page but the drafts — `here`, the page in view, even if it is one,
    /// as every gather across pages does.
    Pages { here: NodeId },
}

/// One match, and what stands between it and a replace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub node: NodeId,
    pub page: NodeId,
    /// The matched bytes of the layer's stored content.
    pub range: Range<usize>,
    /// The layer or one above it is locked: listed, never written.
    pub locked: bool,
    /// The layer or one above it is hidden: still written.
    pub hidden: bool,
    /// The instance the text is a copy inside, whose text a replace overrides.
    pub instance: Option<NodeId>,
    /// The main the text is part of, and how many instances follow it.
    pub main: Option<(NodeId, usize)>,
}

impl Hit {
    /// Whether a replace writes this match: never when locked, and inside an
    /// instance only when asked to.
    pub fn writable(&self, inside_instances: bool) -> bool {
        !self.locked && (self.instance.is_none() || inside_instances)
    }
}

/// Every match of `query` in the text layers of `scope`: pages in the
/// document's order, then layers in the layers panel's order (top of the stack
/// first), then matches in content order.
pub fn find(doc: &Document, scope: &Scope, query: &Query) -> Vec<Hit> {
    if query.is_empty() {
        return Vec::new();
    }
    let pages: Vec<NodeId> = match scope {
        Scope::Under(roots) => {
            let mut v: Vec<NodeId> = Vec::new();
            for p in roots.iter().filter_map(|r| doc.page_of(*r)) {
                if !v.contains(&p) {
                    v.push(p);
                }
            }
            doc.page_ids().filter(|p| v.contains(p)).collect()
        }
        Scope::Page(p) => vec![*p],
        Scope::Pages { here } => doc
            .pages()
            .iter()
            .filter(|p| !p.draft || p.id == *here)
            .map(|p| p.id)
            .collect(),
    };
    let copies = crate::component::instances_by_main(doc);
    let mut out = Vec::new();
    for page in pages {
        let mut stack = vec![(page, false, false, None, None)];
        while let Some((id, locked, hidden, instance, main)) = stack.pop() {
            let Some(n) = doc.get(id) else { continue };
            let locked = locked || n.locked();
            let hidden = hidden || !doc.shown(id);
            let instance = instance
                .or_else(|| (crate::component::instance_root(doc, id) == Some(id)).then_some(id));
            let main = main.or_else(|| {
                (n.component() && instance.is_none())
                    .then(|| (id, copies.get(&id).map_or(0, Vec::len)))
            });
            let wanted = match scope {
                Scope::Under(roots) => roots
                    .iter()
                    .any(|r| *r == id || crate::query::is_within(doc, id, *r)),
                _ => true,
            };
            if wanted
                && let NodeKind::Text { content, .. } = n.kind()
                && n.text_vars()[crate::variable::TEXT_CONTENT].is_none()
            {
                for (range, _) in query.0.replacements(content, "") {
                    out.push(Hit {
                        node: id,
                        page,
                        range,
                        locked,
                        hidden,
                        instance,
                        main,
                    });
                }
            }
            // Popped last child first: the layers panel's order, top of the stack
            // first.
            for c in n.children() {
                stack.push((*c, locked, hidden, instance, main));
            }
        }
    }
    out
}

/// What [`replace`] made: the edit, and how many matches in how many layers it
/// writes.
#[derive(Clone, Debug, Default)]
pub struct Replaced {
    pub tx: Transaction,
    pub matches: usize,
    pub layers: usize,
}

/// One `SetText` per layer replacing those of `hits` a replace may write
/// ([`Hit::writable`]) with what `query` makes of each. A match no longer in the
/// content as it stands is left alone. The replacement is styled as the match's
/// first character was (`Spans::replaced`).
pub fn replace(doc: &Document, hits: &[Hit], query: &Query, inside_instances: bool) -> Replaced {
    let mut by_node: Vec<(NodeId, Vec<Range<usize>>)> = Vec::new();
    let mut at: HashMap<NodeId, usize> = HashMap::new();
    for h in hits.iter().filter(|h| h.writable(inside_instances)) {
        let i = *at.entry(h.node).or_insert_with(|| {
            by_node.push((h.node, Vec::new()));
            by_node.len() - 1
        });
        by_node[i].1.push(h.range.clone());
    }
    let mut out = Replaced::default();
    for (node, ranges) in by_node {
        let Some(NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            ..
        }) = doc.get(node).map(|n| n.kind())
        else {
            continue;
        };
        let edits: Vec<(Range<usize>, String)> = query
            .0
            .replacements(content, "")
            .into_iter()
            .filter(|(r, _)| ranges.contains(r))
            .collect();
        if edits.is_empty() {
            continue;
        }
        let (mut text, mut spans, mut para) = (content.clone(), spans.clone(), para_spans.clone());
        // Back to front, so each edit's range is still where the match found it.
        for (r, with) in edits.iter().rev() {
            spans = spans.replaced(r.start, r.len(), with.len(), style);
            para = para.edited(r.start, r.len(), with.len(), paragraph);
            text.replace_range(r.clone(), with);
        }
        out.matches += edits.len();
        out.layers += 1;
        out.tx.0.push(Operation::SetText {
            id: node,
            content: text,
            spans,
            para_spans: para,
        });
    }
    out
}
