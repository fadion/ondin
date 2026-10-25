//! The font families a document names, and replacing one everywhere — what the
//! app's *Missing fonts* card lists and does.
//!
//! Family names match without case, as font lookup does. The vault is left out of
//! both: a kit's mains are rewritten by its next update, not by the document.

use crate::node::{CharSpans, NodeKind};
use crate::style::StyleValue;
use crate::typography::{CharAttr, Span};
use crate::variable::{Cell, VarId};
use crate::{Document, NodeId, Operation, Transaction};
use std::collections::BTreeMap;

/// Families, each with the layers set in it.
pub type FamilyUses = Vec<(String, Vec<NodeId>)>;

/// Every family the document's text layers draw with, each with the layers that
/// name it: a layer's own family (or its bound variable's value in every mode)
/// and its runs'. Sorted by name; a name is spelled as first met.
pub fn families_in_use(doc: &Document) -> FamilyUses {
    let mut by_key: BTreeMap<String, (String, Vec<NodeId>)> = BTreeMap::new();
    let mut note = |name: &str, id: NodeId| {
        let entry = by_key
            .entry(name.to_lowercase())
            .or_insert_with(|| (name.to_string(), Vec::new()));
        if entry.1.last() != Some(&id) {
            entry.1.push(id);
        }
    };
    for id in text_layers(doc) {
        let Some(node) = doc.get(id) else { continue };
        let NodeKind::Text { style, spans, .. } = node.kind() else {
            continue;
        };
        match node.text_vars()[1] {
            Some(var) => {
                for v in doc.variables().every_value(var) {
                    if let Some(s) = v.as_str() {
                        note(s, id);
                    }
                }
            }
            None => note(&style.font_family, id),
        }
        for span in spans.as_slice() {
            if let CharAttr::Family(name) = &span.attr {
                note(name, id);
            }
        }
    }
    by_key.into_values().collect()
}

/// Write `to` wherever the document names `from`, as one transaction.
///
/// - A text style of the document's own that sets `from` is rewritten, and its
///   users follow it through the commit (`style::settle`).
/// - A layer's own family is rewritten from that layer's own style, so a copy's
///   other overrides stand. A layer under a text style is not: writing its
///   defaults would detach it, and the style carries the family.
/// - Where a style or a layer's defaults change family, its axis and feature
///   settings go with the old typeface; a run's stay.
/// - Runs naming `from` are rewritten on every layer.
/// - A bound family's variable, and every variable its aliases reach, has every
///   cell holding `from` rewritten; a kit's variable is read-only and is left.
///
/// Commit it through the session's door, which propagates, so instances whose
/// family follows their main's keep following.
pub fn replace_family(doc: &Document, from: &str, to: &str) -> Transaction {
    let from = from.to_lowercase();
    let matches = |name: &str| name.to_lowercase() == from;
    let mut ops = Vec::new();

    for style in doc.styles().iter().filter(|s| s.origin.is_none()) {
        if let StyleValue::Text(t) = &style.value
            && matches(&t.font_family)
        {
            let mut t = t.clone();
            in_family(&mut t, to);
            ops.push(Operation::SetStyleValue {
                id: style.id,
                value: StyleValue::Text(t),
            });
        }
    }

    let mut vars: Vec<VarId> = Vec::new();
    for id in text_layers(doc) {
        let Some(node) = doc.get(id) else { continue };
        let NodeKind::Text { style, spans, .. } = node.kind() else {
            continue;
        };
        if let Some(var) = node.text_vars()[1]
            && !vars.contains(&var)
        {
            vars.push(var);
        }
        let new_spans = replaced_spans(spans, &matches, to, style);
        let own = node.text_vars()[1].is_none()
            && node.styles().text.is_none()
            && matches(&style.font_family);
        if own {
            let mut s = (**style).clone();
            in_family(&mut s, to);
            let spans = new_spans.unwrap_or_else(|| spans.clone());
            ops.push(Operation::SetTextStyle {
                id,
                style: s,
                spans: Some(spans),
            });
        } else if let Some(spans) = new_spans {
            ops.push(Operation::SetTextSpans { id, spans });
        }
    }

    // Every variable an alias in a bound one reaches, since the family is held
    // where the chain ends; `vars` doubles as the seen set, so a loop ends.
    let mut i = 0;
    while let Some(&var) = vars.get(i) {
        i += 1;
        let Some(v) = doc.variables().get(var) else {
            continue;
        };
        for cell in v.cells.values() {
            if let Cell::Alias(next) = cell
                && !vars.contains(next)
            {
                vars.push(*next);
            }
        }
    }

    for var in vars {
        let Some(v) = doc.variables().get(var).filter(|v| v.origin.is_none()) else {
            continue;
        };
        for (mode, cell) in &v.cells {
            if let Cell::String(s) = cell
                && matches(s)
            {
                ops.push(Operation::SetVariableCell {
                    id: var,
                    mode: *mode,
                    cell: Cell::String(to.to_string()),
                });
            }
        }
    }
    Transaction(ops)
}

/// `style` set in `family`, its axis and feature settings cleared: their tags
/// mean something else in the next typeface, as the type panel's family pick has it.
fn in_family(style: &mut crate::typography::TextStyle, family: &str) {
    style.font_family = family.to_string();
    style.variations.clear();
    style.features.clear();
}

/// `spans` with every run of a family matching `from` set in `to`, or `None`
/// when no run names it.
fn replaced_spans(
    spans: &CharSpans,
    matches: &dyn Fn(&str) -> bool,
    to: &str,
    style: &crate::typography::TextStyle,
) -> Option<CharSpans> {
    let mut hit = false;
    let parts: Vec<Span<CharAttr>> = spans
        .as_slice()
        .iter()
        .map(|span| match &span.attr {
            CharAttr::Family(name) if matches(name) => {
                hit = true;
                Span {
                    attr: CharAttr::Family(to.to_string()),
                    ..span.clone()
                }
            }
            _ => span.clone(),
        })
        .collect();
    hit.then(|| CharSpans::from_parts(parts, style))
}

/// Every text layer outside the vault, in no particular order.
fn text_layers(doc: &Document) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![doc.root()];
    while let Some(id) = stack.pop() {
        if doc.in_vault(id) {
            continue;
        }
        let Some(node) = doc.get(id) else { continue };
        if matches!(node.kind(), NodeKind::Text { .. }) {
            out.push(id);
        }
        stack.extend(node.children().iter().copied());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::IdSource;
    use crate::typography::TextStyle;

    fn text(family: &str, run: Option<&str>) -> NodeKind {
        let style = TextStyle {
            font_family: family.into(),
            ..TextStyle::default()
        };
        let spans = match run {
            Some(f) => CharSpans::from_parts(
                vec![Span {
                    start: 0,
                    end: 5,
                    attr: CharAttr::Family(f.into()),
                }],
                &style,
            ),
            None => CharSpans::default(),
        };
        NodeKind::Text {
            content: "hello world".into(),
            style: Box::new(style),
            spans,
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: crate::node::TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        }
    }

    /// A family named as a layer's own and as another layer's run is listed once
    /// with both layers, and replacing it (in another case) leaves it nowhere
    /// while the layers' other families stand.
    ///
    /// flip: `replaced_spans` leaving a matching run as it was → fails at "gone
    /// from the runs too".
    #[test]
    fn a_family_is_listed_with_its_layers_and_replaced_everywhere() {
        let mut ids = IdSource::new(0xFA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (own, run) = (ids.mint(), ids.mint());
        let create = |id, index, kind| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind,
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![
            create(own, 0, text("Gone Sans", None)),
            create(run, 1, text("Inter", Some("Gone Sans"))),
        ]))
        .unwrap();

        let used = families_in_use(&doc);
        let gone = used
            .iter()
            .find(|(name, _)| name == "Gone Sans")
            .expect("listed");
        assert_eq!(gone.1.len(), 2, "both layers: {used:?}");

        doc.apply(&replace_family(&doc, "gone sans", "Inter"))
            .unwrap();
        let used = families_in_use(&doc);
        assert_eq!(
            used.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            ["Inter"],
            "gone from the runs too: {used:?}"
        );
        assert!(
            replace_family(&doc, "Gone Sans", "Inter").0.is_empty(),
            "nothing left to replace"
        );
    }

    /// A family bound through an alias is replaced in the variable that holds
    /// it, the alias left standing.
    ///
    /// flip: `replace_family` rewriting only the bound variable's own cells →
    /// fails at "the held cell is rewritten".
    #[test]
    fn a_family_reached_through_an_alias_is_replaced_where_it_is_held() {
        use crate::variable::{Mode, TEXT_FAMILY, VarKind, Variable, text_vars_at};
        let mut ids = IdSource::new(0xFB);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let heading = ids.mint();
        let mode = ids.mint_mode();
        let (brand, alias) = (ids.mint_var(), ids.mint_var());
        let var = |id, name: &str, cell| Variable {
            id,
            name: name.into(),
            kind: VarKind::String,
            cells: [(mode, cell)].into(),
            origin: None,
            forked: None,
            description: String::new(),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: heading,
                parent: root,
                index: 0,
                kind: text("Inter", None),
                transform: None,
                name: None,
            },
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Base".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: var(brand, "Brand/Display", Cell::String("Gone Sans".into())),
            },
            Operation::AddVariable {
                var: var(alias, "Heading font", Cell::Alias(brand)),
            },
        ]))
        .unwrap();
        let bind = crate::build::bind_text_ops(&doc, heading, text_vars_at(TEXT_FAMILY, alias));
        doc.apply(&Transaction(bind)).unwrap();
        assert!(
            families_in_use(&doc).iter().any(|(n, _)| n == "Gone Sans"),
            "the fixture lists the aliased family"
        );

        doc.apply(&replace_family(&doc, "Gone Sans", "Inter"))
            .unwrap();
        let cell = |v| doc.variables().get(v).unwrap().cells[&mode].clone();
        assert_eq!(
            cell(brand),
            Cell::String("Inter".into()),
            "the held cell is rewritten"
        );
        assert_eq!(cell(alias), Cell::Alias(brand), "the alias stands");
        assert!(
            !families_in_use(&doc).iter().any(|(n, _)| n == "Gone Sans"),
            "and the family is no longer listed"
        );
    }
}
