//! The reads that list what a document defines and render what it exports:
//! `run_exports`, `export_code`, `list_components`, `list_variables` and
//! `list_styles` (§8.4). Files come back with their names and are never written.

use super::Answer;
use crate::session::EditorSession;
use ondin_core::{Document, NodeId};
use ondin_mcp::wire::Content;
use serde_json::{Value, json};

/// A file's media type, from its name.
fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "ico" => "image/x-icon",
        "icns" => "image/icns",
        "html" => "text/html",
        "css" => "text/css",
        "js" | "jsx" | "mjs" => "application/javascript",
        "vue" | "txt" | "md" => "text/plain",
        "json" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Render the export settings on `ids`, or on every layer that has some,
/// through the plan *Export all* and `ondin export --all` use.
pub(super) fn run_exports(s: &EditorSession, ids: Option<Vec<NodeId>>) -> Result<Answer, String> {
    let subjects = match ids {
        Some(ids) => ondin_core::build::all_in_document_order(&s.doc, &ids),
        None => crate::panels::exportable_layers_of(&s.doc),
    };
    let opts = ondin_export::PlanOptions {
        folders_from_names: false,
        scale_override: None,
        unset: None,
    };
    let plan = ondin_export::plan(&s.doc, &s.resolved, &subjects, opts);
    if plan.is_empty() {
        return Err("No layer has export settings: add them with `set_exports`.".into());
    }
    let mut listed = Vec::new();
    let mut files = Vec::new();
    for file in &plan {
        let entry = match ondin_export::plan::render(&s.doc, &s.resolved, file) {
            Some(bytes) => {
                let entry = json!({ "path": file.path, "layer": file.node.to_wire(), "bytes": bytes.len() });
                files.push(Content::File {
                    name: file.path.clone(),
                    mime: mime(&file.path),
                    bytes,
                });
                entry
            }
            None => json!({ "path": file.path, "layer": file.node.to_wire(), "failed": true }),
        };
        listed.push(entry);
    }
    Ok(Answer::Files(json!({ "files": listed }), files))
}

/// Layers as code, with the three lists of what it could not say exactly.
pub(super) fn export_code(
    s: &EditorSession,
    args: &Value,
    ids: Option<Vec<NodeId>>,
) -> Result<Answer, String> {
    use ondin_export::code::Target;
    let target = match args.get("target").and_then(Value::as_str) {
        Some("html") => Target::Html,
        Some("react") => Target::React,
        Some("vue") => Target::Vue,
        _ => return Err("`target` is one of html, react, vue".into()),
    };
    let origins = match ids {
        Some(ids) => ids,
        None if !s.selection.ids().is_empty() => s.selection.ids().to_vec(),
        None => s
            .doc
            .get(s.doc.first_page())
            .map(|p| p.children().to_vec())
            .unwrap_or_default(),
    };
    if let Some(missing) = origins.iter().find(|id| s.doc.get(**id).is_none()) {
        return Err(format!("no layer `{}`", missing.to_wire()));
    }
    let origins = ondin_core::build::in_document_order(&s.doc, &origins);
    let export = target.export(&s.doc, &s.resolved, &origins);
    let report = json!({
        "files": export.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
        "approximated": export.approximated,
        "positioned": export.positioned,
        "inlined": export.inlined,
    });
    let files = export
        .files
        .into_iter()
        .map(|f| Content::File {
            mime: mime(&f.path),
            name: f.path,
            bytes: f.bytes,
        })
        .collect();
    Ok(Answer::Files(report, files))
}

/// Every main component and variant set, in document order.
pub(super) fn components(doc: &Document) -> Value {
    let mut out = Vec::new();
    for page in doc.page_ids() {
        let mut stack = vec![page];
        while let Some(id) = stack.pop() {
            let Some(node) = doc.get(id) else { continue };
            let set = ondin_core::variant::is_set(doc, id);
            if set || node.component() {
                let mut entry = json!({
                    "id": id.to_wire(),
                    "name": node.name(),
                    "kind": if set { "set" } else { "main" },
                    "page": page.to_wire(),
                    "description": node.description(),
                    "doc_link": node.doc_link(),
                });
                if set {
                    entry["properties"] = serde_json::to_value(node.props()).unwrap_or(Value::Null);
                } else if let Some(of) = ondin_core::variant::set_of(doc, id) {
                    entry["set"] = json!(of.to_wire());
                    entry["variant"] = json!(node.variant());
                }
                out.push(entry);
            }
            stack.extend(node.children().iter().rev().copied());
        }
    }
    json!({ "components": out })
}

/// A resolved value as JSON: a colour as `#rrggbbaa`.
fn value(v: ondin_core::variable::Value) -> Value {
    use ondin_core::variable::Value as V;
    match v {
        V::Color(c) => {
            let [r, g, b, a] = c.to_rgba8().to_u8_array();
            json!(format!("#{r:02x}{g:02x}{b:02x}{a:02x}"))
        }
        V::Number(n) => json!(n),
        V::Boolean(b) => json!(b),
        V::String(s) => json!(s),
    }
}

/// Every collection with its modes, and every variable with its cells as
/// written and the value each mode resolves it to.
pub(super) fn variables(doc: &Document) -> Value {
    let vars = doc.variables();
    let collections: Vec<Value> = vars
        .collections()
        .iter()
        .map(|c| {
            json!({
                "id": serde_json::to_value(c.id).unwrap_or(Value::Null),
                "name": c.name,
                "modes": c.modes.iter().map(|m| json!({
                    "id": serde_json::to_value(m.id).unwrap_or(Value::Null),
                    "name": m.name,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let variables: Vec<Value> = vars
        .iter()
        .map(|v| {
            let mut entry = serde_json::to_value(v).unwrap_or(Value::Null);
            if let Some(c) = vars.collection_of(v.id) {
                entry["collection"] = serde_json::to_value(c.id).unwrap_or(Value::Null);
                let resolved: serde_json::Map<String, Value> = c
                    .modes
                    .iter()
                    .filter_map(|m| {
                        let id = serde_json::to_value(m.id).ok()?.as_str()?.to_owned();
                        Some((id, value(vars.resolve(v.id, m.id)?)))
                    })
                    .collect();
                entry["resolved"] = Value::Object(resolved);
            }
            entry["users"] = json!(ondin_core::variable::users(doc, v.id).len());
            entry
        })
        .collect();
    json!({ "collections": collections, "variables": variables })
}

/// Every style with its slot and how many layers use it.
pub(super) fn styles(doc: &Document) -> Value {
    let styles: Vec<Value> = doc
        .styles()
        .iter()
        .map(|s| {
            let mut entry = serde_json::to_value(s).unwrap_or(Value::Null);
            entry["slot"] = json!(format!("{:?}", s.slot()).to_lowercase());
            entry["users"] = json!(ondin_core::style::users(doc, s.id).len());
            entry
        })
        .collect();
    json!({ "styles": styles })
}
