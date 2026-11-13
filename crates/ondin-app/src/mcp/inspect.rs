//! The reads that answer a question about a document rather than list what it
//! holds (§8.4, Phase 1): layers like one, contrast, near-identical colours, the
//! design tokens. Each is the app's own check, run on the session.

use super::{Answer, node, nodes};
use crate::input::Same;
use crate::session::EditorSession;
use ondin_core::{Document, NodeId};
use serde_json::{Value, json};

/// The tools this module answers.
pub(super) const TOOLS: [&str; 4] = [
    "find_like",
    "check_contrast",
    "review_colours",
    "export_tokens",
];

/// Answer `tool` on `s`.
pub(super) fn run(s: &EditorSession, tool: &str, args: &Value) -> Result<Answer, String> {
    match tool {
        "find_like" => find_like(s, args),
        "check_contrast" => check_contrast(s, args),
        "review_colours" => review_colours(s, args),
        "export_tokens" => {
            let t = ondin_export::dtcg::tokens(&s.doc, None);
            Ok(Answer::Json(json!({ "tokens": t.json, "notes": t.notes })))
        }
        tool => Err(format!("no tool `{tool}`")),
    }
}

/// Every layer like `id` by `by`: sharing its fills, strokes, font or effects
/// (*Select same*), the same layer in each copy of its block (*Select matching
/// layers*), or a main's or set's instances (*Select all instances*). The
/// source is listed too; the vault's mains never are.
fn find_like(s: &EditorSession, args: &Value) -> Result<Answer, String> {
    let doc = &s.doc;
    let id = super::required(args, "id")?;
    if doc.get(id).is_none() {
        return Err(format!("no layer `{}`", id.to_wire()));
    }
    let by = args.get("by").and_then(Value::as_str).ok_or("needs `by`")?;
    let found: Vec<NodeId> = match by {
        "instances" => {
            let mains = match ondin_core::variant::is_set(doc, id) {
                true => ondin_core::variant::variants(doc, id),
                false if doc.get(id).is_some_and(|n| n.component()) => vec![id],
                false => return Err("`instances` needs a main component or a set".into()),
            };
            mains
                .iter()
                .flat_map(|m| ondin_core::component::instances_of(doc, *m))
                .collect()
        }
        "matching" => {
            let page = doc.page_of(id).ok_or("the layer is on no page")?;
            ondin_core::matching::matching_layers(doc, page, &[id])
        }
        same => {
            let same = match same {
                "fill" => Same::Fill,
                "stroke" => Same::Stroke,
                "font" => Same::Font,
                "effect" => Same::Effect,
                _ => {
                    return Err(
                        "`by` is one of fill, stroke, font, effect, matching, instances".into(),
                    );
                }
            };
            if !crate::app::has_same(doc, id, same) {
                return Err(format!("The layer has no {by} to match."));
            }
            let mut found = Vec::new();
            let mut stack: Vec<NodeId> = doc.page_ids().collect();
            stack.reverse();
            while let Some(at) = stack.pop() {
                if Some(at) == doc.vault() {
                    continue;
                }
                let Some(n) = doc.get(at) else { continue };
                if !doc.is_page(at) && crate::app::shares(doc, id, at, same) {
                    found.push(at);
                }
                stack.extend(n.children().iter().rev().copied());
            }
            found
        }
    };
    let listed: Vec<Value> = found
        .iter()
        .filter_map(|i| {
            let n = doc.get(*i)?;
            Some(json!({
                "id": i.to_wire(),
                "name": n.name(),
                "page": doc.page_of(*i).map(NodeId::to_wire),
            }))
        })
        .collect();
    Ok(Answer::Json(json!({ "nodes": listed })))
}

/// The contrast check (*Check contrast*, `ondin check contrast`): each shown
/// text layer, and with `non_text` each small shape, measured against what is
/// drawn behind it in every mode. `ids` limits it to the layers under them; else
/// a page, the first when none is named.
fn check_contrast(s: &EditorSession, args: &Value) -> Result<Answer, String> {
    use ondin_export::contrast::{self, Level, Options};
    let level = match args.get("level").and_then(Value::as_str) {
        None | Some("aa") => Level::Aa,
        Some("aaa") => Level::Aaa,
        Some("apca") => Level::Apca,
        Some(other) => return Err(format!("`level` is aa, aaa or apca, not `{other}`")),
    };
    let options = Options {
        level,
        non_text: args.get("non_text").and_then(Value::as_bool) == Some(true),
        default_ground: None,
    };
    let doc = &s.doc;
    let scope = nodes(args, "ids")?;
    let pages: Vec<NodeId> = match &scope {
        Some(ids) => {
            if let Some(gone) = ids.iter().find(|i| doc.get(**i).is_none()) {
                return Err(format!("no layer `{}`", gone.to_wire()));
            }
            let mut pages: Vec<NodeId> = ids.iter().filter_map(|i| doc.page_of(*i)).collect();
            pages.sort_unstable();
            pages.dedup();
            pages
        }
        None => vec![super::page_of(s, node(args, "page")?)?],
    };
    let mut findings = Vec::new();
    let mut skipped = Vec::new();
    for page in pages {
        let only: Option<Vec<NodeId>> = scope.as_ref().map(|ids| {
            contrast::subjects(doc, &s.resolved, page, options.non_text)
                .into_iter()
                .filter(|sub| ids.iter().any(|r| under(doc, *sub, *r)))
                .collect()
        });
        let report = contrast::check_with(doc, page, options, only.as_deref());
        for f in &report.findings {
            findings.push(json!({
                "id": f.id.to_wire(),
                "trail": f.trail,
                "passes": f.passes(),
                "score": level.format(f.ratio),
                "needs": level.format_needs(f.needs),
                "large": f.large,
                "non_text": f.non_text,
                "mode": f.mode,
            }));
        }
        for k in &report.skipped {
            skipped.push(json!({ "id": k.id.to_wire(), "trail": k.trail, "why": k.why }));
        }
    }
    let failing = findings.iter().filter(|f| f["passes"] == false).count();
    Ok(Answer::Json(json!({
        "level": level.label(),
        "measured": findings.len(),
        "failing": failing,
        "findings": findings,
        "skipped": skipped,
    })))
}

/// Whether `id` is `root` or under it.
fn under(doc: &Document, id: NodeId, root: NodeId) -> bool {
    let mut at = Some(id);
    while let Some(i) = at {
        if i == root {
            return true;
        }
        at = doc.get(i).and_then(|n| n.parent());
    }
    false
}

/// *Colour review*: every distinct colour of the layers under `ids`, or of
/// every page, most-used first, and the groups of near-identical ones that
/// `merge_colours` takes, at a threshold.
fn review_colours(s: &EditorSession, args: &Value) -> Result<Answer, String> {
    use ondin_core::color_review::{self, Threshold};
    let doc = &s.doc;
    let threshold = match args.get("threshold").and_then(Value::as_str) {
        Some("tight") => Threshold::Tight,
        None | Some("close") => Threshold::Close,
        Some("loose") => Threshold::Loose,
        Some(other) => {
            return Err(format!(
                "`threshold` is tight, close or loose, not `{other}`"
            ));
        }
    };
    let scope = nodes(args, "ids")?;
    let roots = color_review::roots(doc, scope.as_deref());
    let census = color_review::census(doc, &roots);
    let swatch = |i: usize| {
        let w = &census[i];
        json!({
            "colour": super::reads::hex(w.color),
            "variable": w.var.map(|v| doc.variables().get(v).map_or_else(String::new, |v| v.name.clone())),
            "uses": w.uses,
            "layers": w.nodes.len(),
        })
    };
    let groups: Vec<Value> = color_review::groups(&census, threshold)
        .iter()
        .map(|g| json!({ "colours": g.members.iter().map(|m| swatch(*m)).collect::<Vec<_>>() }))
        .collect();
    Ok(Answer::Json(json!({
        "colours": (0..census.len()).map(swatch).collect::<Vec<_>>(),
        "groups": groups,
    })))
}
