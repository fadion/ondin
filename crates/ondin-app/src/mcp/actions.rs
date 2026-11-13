//! The app's own actions an agent may take that are no registry verb (§8.4,
//! Phase 4): importing design tokens, and keeping and listing the document's
//! versions. Tokens land as one undo step through the commit door; a version is
//! a file the library pins, outside history.

use super::{Answer, Documents, commit};
use crate::session::EditorSession;
use ondin_mcp::queue::Call;
use serde_json::{Value, json};
use std::path::Path;

/// The tools this module answers.
pub(super) const TOOLS: [&str; 3] = ["import_tokens", "list_versions", "save_version"];

/// Answer `call` on the document at `path`.
pub(super) fn run(docs: &mut impl Documents, path: &Path, call: &Call) -> Result<Answer, String> {
    let args = &call.args;
    match call.tool.as_str() {
        "import_tokens" => {
            let s = docs.session(path).ok_or("the document closed")?;
            import_tokens(s, &call.client, args)
        }
        "list_versions" => list_versions(docs, path),
        "save_version" => save_version(docs, path, &call.client, args),
        tool => Err(format!("no tool `{tool}`")),
    }
}

/// A DTCG token file read into the document's variables and styles, every row
/// the app's review starts ticked applied as one undo step (removals only when
/// asked), as *Import tokens* applies them.
fn import_tokens(s: &mut EditorSession, client: &str, args: &Value) -> Result<Answer, String> {
    use ondin_export::dtcg_in::{self, Change};
    let text = match args.get("tokens") {
        Some(Value::String(t)) => t.clone(),
        Some(v @ Value::Object(_)) => v.to_string(),
        _ => return Err("needs `tokens`: the DTCG file's JSON".into()),
    };
    let file = dtcg_in::parse(&text).map_err(|r| format!("Not design tokens: {r}"))?;
    let t = s.doc.variables();
    let into = match args.get("collection").and_then(Value::as_str) {
        None => t.first_collection().id,
        Some(name) => t
            .collections()
            .iter()
            .find(|c| c.name == name)
            .map(|c| c.id)
            .ok_or_else(|| format!("no collection called `{name}`"))?,
    };
    let review = dtcg_in::review(&s.doc, &file, into, &mut s.ids)
        .map_err(|r| format!("The tokens cannot be read: {r}"))?;
    let removals = args.get("removals").and_then(Value::as_bool) == Some(true);
    let ticked: Vec<bool> = review
        .rows
        .iter()
        .map(|r| r.ticked || (removals && r.change == Change::Removed))
        .collect();
    let ops = review.ops(&s.doc, &ticked)?;
    let count = |c: Change| {
        review
            .rows
            .iter()
            .zip(&ticked)
            .filter(|(r, on)| **on && r.change == c)
            .count()
    };
    let report = json!({
        "added": count(Change::Added),
        "changed": count(Change::Changed),
        "renamed": count(Change::Renamed),
        "removed": count(Change::Removed),
        "kit": review.rows.iter().filter(|r| r.change == Change::Kit).count(),
        "left_out": review
            .left_out
            .iter()
            .map(|l| json!({ "token": l.path, "type": l.ty }))
            .collect::<Vec<_>>(),
    });
    if !ops.is_empty() {
        commit(s, client, "import_tokens", ondin_core::Transaction(ops))?;
    }
    Ok(Answer::Json(report))
}

/// The library entry of the document at `path`, refused for a file the library
/// does not list or one with no id to keep versions under.
fn entry(docs: &mut impl Documents, path: &Path) -> Result<crate::library::scan::Entry, String> {
    let library = docs.library().ok_or(super::kits::NO_LIBRARY)?;
    library
        .entry_at(path)
        .filter(|e| {
            e.meta
                .id
                .as_deref()
                .is_some_and(crate::library::ids::is_minted)
        })
        .cloned()
        .ok_or_else(|| "Versions are kept only for documents in the library.".into())
}

/// The document's pinned versions, newest first: when, and the name given.
fn list_versions(docs: &mut impl Documents, path: &Path) -> Result<Answer, String> {
    let entry = entry(docs, path)?;
    let root = docs.library().ok_or(super::kits::NO_LIBRARY)?.root.clone();
    let id = entry.meta.id.as_deref().unwrap_or_default();
    let versions: Vec<Value> = crate::library::store::versions(&root, id)
        .iter()
        .map(|v| {
            json!({
                "at": crate::versions::stamp_of(v),
                "name": crate::library::store::version_label(v),
            })
        })
        .collect();
    Ok(Answer::Json(json!({ "versions": versions })))
}

/// Save the document and pin the file as a version, named when a name is given:
/// a point the person can return to from *Version history*. Outside undo.
fn save_version(
    docs: &mut impl Documents,
    path: &Path,
    client: &str,
    args: &Value,
) -> Result<Answer, String> {
    // Refused before the save: a loose file is not written for a version it
    // cannot keep.
    entry(docs, path)?;
    docs.save(path)?;
    let entry = entry(docs, path)?;
    let root = docs.library().ok_or(super::kits::NO_LIBRARY)?.root.clone();
    let pinned = crate::library::store::pin_version(&root, &entry)
        .map_err(|e| format!("The version was not kept: {e}"))?;
    // `entry` checked the id `pin_version` answers `None` without.
    let Some(version) = pinned else {
        return Err("The version was not kept.".into());
    };
    let name = args.get("name").and_then(Value::as_str).unwrap_or_default();
    if !name.trim().is_empty() {
        crate::library::store::set_version_label(&version, name)
            .map_err(|e| format!("The version was kept, but not its name: {e}"))?;
    }
    let said = match name.trim() {
        "" => "Saved a version".to_string(),
        n => format!("Saved version “{n}”"),
    };
    docs.tell(format!("{said}{}", super::kits::by(client)));
    Ok(Answer::Json(json!({
        "at": crate::versions::stamp_of(&version),
        "name": crate::library::store::version_label(&version),
    })))
}

#[cfg(test)]
mod tests {
    use super::super::serve::Headless;
    use super::super::{Documents, Engine};
    use ondin_mcp::queue::Call;
    use ondin_mcp::wire::Output;
    use serde_json::{Value, json};

    fn send(engine: &mut Engine, docs: &mut Headless, tool: &str, args: Value) -> Output {
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        engine.handle(docs, Call::new(1, None, tool, args, reply))
    }

    /// `save_version` keeps a named version a library document lists after it;
    /// a document outside the library is refused.
    ///
    /// flip: `save_version` skipping `set_version_label` → fails at the name.
    #[test]
    fn a_saved_version_is_listed_with_its_name() {
        let root = crate::kits::picker::tests::temp_root("mcp-versions");
        let mut engine = Engine::default();
        let mut docs = Headless::new(Some(root.clone()));
        let out = send(
            &mut engine,
            &mut docs,
            "create_document",
            json!({ "name": "Site" }),
        );
        let path = out.structured.unwrap()["path"].as_str().unwrap().to_owned();
        assert!(
            !send(
                &mut engine,
                &mut docs,
                "use_document",
                json!({ "document": path })
            )
            .is_error
        );
        let saved = send(
            &mut engine,
            &mut docs,
            "save_version",
            json!({ "name": "Before the redesign" }),
        );
        assert!(!saved.is_error, "{:?}", saved.content);
        let listed = send(&mut engine, &mut docs, "list_versions", json!({}));
        let versions = listed.structured.unwrap()["versions"].clone();
        assert_eq!(versions.as_array().map(Vec::len), Some(1), "{versions}");
        assert_eq!(versions[0]["name"], "Before the redesign");

        let loose = std::env::temp_dir().join(format!("ondin-loose-{}.ondin", std::process::id()));
        crate::library::store::write_document(
            &loose,
            &ondin_core::Document::new(ondin_core::IdSource::new(1).mint()),
        )
        .unwrap();
        let mut docs_loose = Headless::new(Some(root.clone()));
        assert!(docs_loose.open(&loose.display().to_string()).is_ok());
        let mut engine = Engine::default();
        send(
            &mut engine,
            &mut docs_loose,
            "use_document",
            json!({ "document": loose }),
        );
        assert!(send(&mut engine, &mut docs_loose, "save_version", json!({})).is_error);
        let _ = std::fs::remove_file(&loose);
        let _ = std::fs::remove_dir_all(&root);
    }
}
