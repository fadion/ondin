//! Phase 4 (§8.4): kits and the library over MCP.
//!
//! The app's kit verbs act on the tab in front; an agent's document may be a
//! tab behind or a `serve` session, so these run on that document's session and
//! the library the host lends, through the core verbs the app's own run on and
//! the app's wording for what they say. A kit is named by its document id or
//! its name; the library's state is read again for every call.

use super::{Answer, Documents, READ_ONLY, commit, required};
use crate::kits::index::{KitEntry, kits_in};
use crate::kits::picker::{Source, build_placement};
use crate::kits::updates::{stack_local, waiting};
use crate::library::state::Library;
use crate::session::EditorSession;
use ondin_core::kit::{KitError, Published};
use ondin_core::{Document, NodeId};
use ondin_mcp::queue::Call;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// The Phase 4 tools that need no current document.
pub(super) const WITHOUT_DOCUMENT: [&str; 3] = ["list_documents", "list_kits", "create_document"];

/// Whether `tool` is a Phase 4 tool run on the current document.
pub(super) fn is_kit_tool(tool: &str) -> bool {
    ondin_mcp::tools::PHASE_4.contains(&tool) && !WITHOUT_DOCUMENT.contains(&tool)
}

const NO_LIBRARY: &str = "There is no library folder; the person sets one in Settings.";

/// The status line's tail naming who acted: the client's name as it gave it.
fn by(client: &str) -> String {
    match client.trim() {
        "" => " · by an agent".to_owned(),
        c => format!(" · by {c}"),
    }
}

/// Answer a Phase 4 tool that needs no current document.
pub(super) fn without_document(docs: &mut impl Documents, call: &Call) -> Result<Value, String> {
    match call.tool.as_str() {
        "list_documents" => Ok(documents(docs.library().ok_or(NO_LIBRARY)?)),
        "list_kits" => {
            let lib = docs.library().ok_or(NO_LIBRARY)?;
            let kits: Vec<Value> = kits_in(lib, None)
                .iter()
                .map(|k| kit_json(lib, k))
                .collect();
            Ok(json!({ "kits": kits }))
        }
        "create_document" => create_document(docs, call),
        tool => Err(format!("no tool `{tool}`")),
    }
}

/// Answer a Phase 4 tool on the document at `path`.
pub(super) fn run(docs: &mut impl Documents, path: &Path, call: &Call) -> Result<Answer, String> {
    let args = &call.args;
    let json = |v: Value| Ok(Answer::Json(v));
    match call.tool.as_str() {
        "make_kit" | "publish_kit" | "stop_sharing_kit" => mark(docs, path, call),
        tool => {
            let (s, lib) = docs
                .with_library(path)
                .ok_or_else(|| format!("{} is no longer open", path.display()))?;
            match tool {
                "get_kit" => json(get_kit(&s.doc, lib.ok_or(NO_LIBRARY)?, args)?),
                "list_kit_updates" => json(updates(&s.doc, lib.ok_or(NO_LIBRARY)?)),
                "insert_from_kit" => insert(s, lib.ok_or(NO_LIBRARY)?, call),
                "accept_kit_updates" => accept(s, lib.ok_or(NO_LIBRARY)?, call),
                "make_local" => make_local(s, call),
                "map_kit_modes" => map_modes(s, call),
                tool => Err(format!("no tool `{tool}`")),
            }
        }
    }
}

/// Every document in the library.
fn documents(lib: &Library) -> Value {
    let list: Vec<Value> = lib
        .entries
        .iter()
        .map(|e| {
            json!({
                "path": e.path.display().to_string(),
                "name": e.display_name(),
                "project": lib.project_of(e).map(|p| p.name.clone()),
                "kit": e.meta.kit.as_ref().is_some_and(|k| k.is_shared()),
            })
        })
        .collect();
    json!({ "documents": list })
}

/// A kit's name as its file has it, not as private mode draws it.
fn real_name(lib: &Library, k: &KitEntry) -> String {
    lib.entry_at(&k.path)
        .map_or_else(|| k.name.clone(), |e| e.display_name())
}

fn kit_json(lib: &Library, k: &KitEntry) -> Value {
    json!({
        "id": k.id,
        "name": real_name(lib, k),
        "path": k.path.display().to_string(),
        "version": k.version,
        "note": k.note,
    })
}

fn text<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{name}` is required"))
}

/// The library's kit `wanted` names, by id or by name, never `doc`'s own.
fn find_kit(lib: &Library, doc: &Document, wanted: &str) -> Result<KitEntry, String> {
    let kits = kits_in(lib, doc.meta().id.as_deref());
    if let Some(k) = kits.iter().find(|k| k.id == wanted) {
        return Ok(k.clone());
    }
    let named: Vec<&KitEntry> = kits
        .iter()
        .filter(|k| real_name(lib, k).eq_ignore_ascii_case(wanted.trim()))
        .collect();
    match named.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!(
            "No kit `{wanted}` in the library; `list_kits` names them"
        )),
        many => Err(format!(
            "Several kits are called `{wanted}`; name one by id: {}",
            many.iter()
                .map(|k| k.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `k`'s published state and the version its file says it published (§15
/// D1023: the scan's number may be older than the file read).
fn read_published(lib: &Library, k: &KitEntry) -> Result<(u32, Arc<Published>), String> {
    let name = real_name(lib, k);
    let doc = std::fs::read(&k.path)
        .ok()
        .and_then(|b| ondin_core::io::load(&b).ok())
        .ok_or_else(|| format!("{name} could not be read"))?;
    let version = doc.meta().kit.as_ref().map_or(0, |m| m.version);
    let published = doc
        .published()
        .ok_or_else(|| format!("{name} has published nothing yet"))?;
    Ok((version, Arc::new(published.clone())))
}

/// A kit's published components, variables and styles: what `insert_from_kit`
/// places and what accepting its updates brings.
fn get_kit(doc: &Document, lib: &Library, args: &Value) -> Result<Value, String> {
    let k = find_kit(lib, doc, text(args, "kit")?)?;
    let (version, published) = read_published(lib, &k)?;
    let src = published.document();
    let mut out = kit_json(lib, &k);
    out["version"] = json!(version);
    out["components"] = super::reads::components(src)["components"].take();
    out["variables"] = super::reads::variables(src);
    out["styles"] = super::reads::styles(src)["styles"].take();
    Ok(out)
}

/// The kits `doc` uses with a newer publish waiting.
fn updates(doc: &Document, lib: &Library) -> Value {
    let list: Vec<Value> = waiting(doc, lib, true)
        .iter()
        .map(|w| {
            json!({
                "kit": w.kit.id,
                "name": real_name(lib, &w.kit),
                "held": w.held,
                "version": w.kit.version,
                "note": w.kit.note,
                "components": w.mains,
                "instances": w.instances,
            })
        })
        .collect();
    json!({ "updates": list })
}

/// Place an instance of a kit's published main, vendoring it first.
fn insert(s: &mut EditorSession, lib: &Library, call: &Call) -> Result<Answer, String> {
    let args = &call.args;
    if s.compare.is_some() {
        return Err(READ_ONLY.to_owned());
    }
    let k = find_kit(lib, &s.doc, text(args, "kit")?)?;
    let (version, published) = read_published(lib, &k)?;
    let main = required(args, "main")?;
    let placeable = published
        .document()
        .get(main)
        .is_some_and(|n| n.component() || n.set().is_some());
    if !placeable {
        return Err(format!(
            "`{}` is no published component of {}; `get_kit` lists them",
            main.to_wire(),
            real_name(lib, &k)
        ));
    }
    let parent = required(args, "parent")?;
    if s.doc.get(parent).is_none() {
        return Err(format!("no layer `{}`", parent.to_wire()));
    }
    let number = |name| args.get(name).and_then(Value::as_f64);
    let centre = match (number("x"), number("y")) {
        (None, None) => None,
        (x, y) => {
            let into = s
                .resolved
                .world_transform(parent)
                .unwrap_or(ondin_core::kurbo::Affine::IDENTITY);
            Some(into * ondin_core::kurbo::Point::new(x.unwrap_or(0.0), y.unwrap_or(0.0)))
        }
    };
    let source = Source::Kit {
        id: k.id.clone(),
        version,
        published,
        top: main,
    };
    let (tx, instance) = build_placement(&s.doc, &mut s.ids, &source, parent, centre, None)?;
    commit(s, &call.client, &call.tool, tx)?;
    Ok(Answer::Json(json!({ "created": [instance.to_wire()] })))
}

/// Accept the waiting updates, every kit's or one's, in one undo step.
fn accept(s: &mut EditorSession, lib: &Library, call: &Call) -> Result<Answer, String> {
    if s.compare.is_some() {
        return Err(READ_ONLY.to_owned());
    }
    let only = match call.args.get("kit").and_then(Value::as_str) {
        Some(wanted) => Some(find_kit(lib, &s.doc, wanted)?.id),
        None => None,
    };
    let all = waiting(&s.doc, lib, false);
    let chosen: Vec<_> = all
        .iter()
        .filter(|w| only.as_deref().is_none_or(|k| k == w.kit.id))
        .collect();
    if chosen.is_empty() {
        return Ok(Answer::Json(json!({ "accepted": [] })));
    }
    let mut loaded = Vec::new();
    for w in &chosen {
        // Nothing is applied while one kit cannot be read: kits reach each
        // other's mains, so a partial accept is the riskier answer.
        let (version, p) =
            read_published(lib, &w.kit).map_err(|why| format!("Nothing was updated: {why}"))?;
        loaded.push((w.kit.id.clone(), version, p));
    }
    let kits: Vec<(&str, u32, &Published)> = loaded
        .iter()
        .map(|(k, v, p)| (k.as_str(), *v, p.as_ref()))
        .collect();
    let page = s.page;
    let content = s.preview_world_bounds(page);
    let update =
        ondin_core::kit::update_all(&s.doc, &mut s.ids, page, &kits).map_err(|e| match e {
            KitError::NeedsUpdate(_) | KitError::Modes(_) if only.is_some() && all.len() > 1 => {
                "This update needs another kit's too; accept every kit's by leaving out `kit`"
                    .to_owned()
            }
            KitError::NeedsUpdate(_) | KitError::Modes(_) => {
                "This update needs a newer version of another kit it uses, which is not in the \
                 library"
                    .to_owned()
            }
            KitError::Circular(_) => {
                "This update uses this document's own components, so it cannot be taken here"
                    .to_owned()
            }
            _ => "The kit update could not be applied: its published components are incomplete"
                .to_owned(),
        })?;
    let mut tx = update.tx;
    tx.0.extend(stack_local(&s.doc, &tx, &update.made_local, content));
    commit(s, &call.client, &call.tool, tx)?;
    let ids = |v: &[NodeId]| v.iter().map(|i| i.to_wire()).collect::<Vec<_>>();
    Ok(Answer::Json(json!({
        "accepted": loaded.iter().map(|(k, v, _)| json!({ "kit": k, "version": v })).collect::<Vec<_>>(),
        "changed": ids(&update.changed),
        "removed": ids(&update.removed),
        "made_local": ids(&update.made_local),
    })))
}

/// The vault top a vendored main is reached by: `id` itself or an ancestor in
/// the vault, or for a layer in an instance the top of the main it shows.
fn vault_top(doc: &Document, id: NodeId) -> Option<NodeId> {
    let vault = doc.vault()?;
    let mut at = match doc.in_vault(id) {
        true => id,
        false => {
            ondin_core::component::main_of(doc, ondin_core::component::instance_root(doc, id)?)?
        }
    };
    loop {
        let parent = doc.get(at)?.parent()?;
        if parent == vault {
            return Some(at);
        }
        at = parent;
    }
}

/// Make kit components the document's own mains, stacked beside the content.
fn make_local(s: &mut EditorSession, call: &Call) -> Result<Answer, String> {
    if s.compare.is_some() {
        return Err(READ_ONLY.to_owned());
    }
    let wanted = super::nodes(&call.args, "ids")?.ok_or("`ids` is required")?;
    let mut tops: Vec<NodeId> = Vec::new();
    for id in wanted {
        let top = vault_top(&s.doc, id).ok_or_else(|| {
            format!(
                "`{}` is no kit component, nor an instance of one",
                id.to_wire()
            )
        })?;
        if !tops.contains(&top) {
            tops.push(top);
        }
    }
    let page = s.page;
    let content = s.preview_world_bounds(page);
    let made = ondin_core::kit::make_local_report(&s.doc, &mut s.ids, page, &tops);
    let mut tx = made.tx;
    if tx.0.is_empty() {
        return Ok(Answer::Json(json!({ "made_local": [] })));
    }
    tx.0.extend(stack_local(&s.doc, &tx, &tops, content));
    commit(s, &call.client, &call.tool, tx)?;
    Ok(Answer::Json(json!({
        "made_local": tops.iter().map(|t| t.to_wire()).collect::<Vec<_>>(),
    })))
}

/// Set which kit mode each of the document's modes reads: the variables
/// browser's mapping row, the kit's held record with only its hand-set map
/// changed.
fn map_modes(s: &mut EditorSession, call: &Call) -> Result<Answer, String> {
    use ondin_core::variable::ModeId;
    if s.compare.is_some() {
        return Err(READ_ONLY.to_owned());
    }
    let wanted = text(&call.args, "kit")?;
    let table = s.doc.variables();
    let held: Vec<(&String, &ondin_core::variable::KitModes)> = table
        .kits()
        .iter()
        .filter(|(id, k)| {
            id.as_str() == wanted
                || k.name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(wanted.trim()))
        })
        .collect();
    let (kit, mut modes) = match held.as_slice() {
        [(id, k)] => ((*id).clone(), (*k).clone()),
        [] => {
            return Err(format!(
                "This document holds no modes of a kit `{wanted}`; `list_variables` shows the \
                 kits' collections"
            ));
        }
        _ => {
            return Err(format!(
                "Several kits are called `{wanted}`; name one by id"
            ));
        }
    };
    let mapping = call
        .args
        .get("mapping")
        .and_then(Value::as_object)
        .ok_or("`mapping` is required: the document's mode ids to the kit's, or null")?;
    let mode = |v: &str| serde_json::from_value::<ModeId>(json!(v)).ok();
    for (from, to) in mapping {
        let from = mode(from)
            .filter(|m| table.owner_of(*m).is_some() && modes.mode(*m).is_none())
            .ok_or_else(|| format!("`{from}` is no mode of this document's own"))?;
        match to.as_str() {
            None if to.is_null() => {
                modes.map.remove(&from);
            }
            Some(to_id) => {
                let to = mode(to_id)
                    .filter(|m| modes.mode(*m).is_some())
                    .ok_or_else(|| format!("`{to_id}` is no mode of that kit"))?;
                modes.map.insert(from, to);
            }
            None => return Err("each mapping value is a kit mode id or null".to_owned()),
        }
    }
    let tx = ondin_core::Transaction(vec![ondin_core::Operation::SetKitModes {
        kit,
        modes: Some(modes),
    }]);
    commit(s, &call.client, &call.tool, tx)?;
    Ok(Answer::Json(json!({})))
}

/// File a new, empty document in the library, in a project when one is named.
fn create_document(docs: &mut impl Documents, call: &Call) -> Result<Value, String> {
    let name = text(&call.args, "name")?.trim();
    if name.is_empty() {
        return Err("`name` is empty".to_owned());
    }
    let lib = docs.library().ok_or(NO_LIBRARY)?;
    if !lib.may_write() {
        return Err(
            "The library folder isn't available; the person checks the base folder in Settings."
                .to_owned(),
        );
    }
    let project = match call.args.get("project").and_then(Value::as_str) {
        None => None,
        Some(wanted) => Some(
            lib.projects
                .projects
                .iter()
                .find(|p| p.id == wanted || p.name.eq_ignore_ascii_case(wanted.trim()))
                .and_then(|p| lib.projects.destination(&p.id))
                .cloned()
                .ok_or_else(|| format!("No open project `{wanted}` in the library"))?,
        ),
    };
    let root = lib.root.clone();
    let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
    let mut doc = Document::new(ids.mint());
    let path = crate::library::store::file_document(&root, project.as_ref(), name, &mut doc)
        .map_err(|e| {
            format!(
                "Could not create the file: {}",
                crate::private::io_error(&e)
            )
        })?;
    docs.tell(format!(
        "Filed {}{}",
        crate::private::document(name, 0),
        by(&call.client)
    ));
    Ok(json!({ "path": path.display().to_string() }))
}

/// `make_kit`, `publish_kit` and `stop_sharing_kit`: the kit mark is document
/// metadata, written and saved at once as the document menu's rows do, outside
/// undo.
fn mark(docs: &mut impl Documents, path: &Path, call: &Call) -> Result<Answer, String> {
    use ondin_core::meta::KitMark;
    let s = docs
        .session(path)
        .ok_or_else(|| format!("{} is no longer open", path.display()))?;
    if s.compare.is_some() {
        return Err(READ_ONLY.to_owned());
    }
    let name = crate::private::document(&s.document_title(), 0).into_owned();
    let mut meta = s.doc.meta().clone();
    let shared = meta.kit.as_ref().is_some_and(|k| k.is_shared());
    let (said, answer) = match call.tool.as_str() {
        "make_kit" => {
            if shared {
                return Err("This document is already a kit".to_owned());
            }
            // A kit is found by its document id, so one without an id is given
            // one, as filing gives it.
            if meta.id.is_none() {
                meta.id = Some(crate::library::ids::mint());
            }
            meta.kit.get_or_insert_with(KitMark::default).sharing = true;
            s.doc.set_meta(meta);
            (
                crate::kits::made_kit_said(&name),
                json!({ "id": s.doc.meta().id }),
            )
        }
        "stop_sharing_kit" => {
            let Some(kit) = meta.kit.as_mut().filter(|k| k.is_shared()) else {
                return Err("This document is not a kit".to_owned());
            };
            kit.sharing = false;
            s.doc.set_meta(meta);
            (crate::kits::stopped_sharing_said(&name), json!({}))
        }
        _ => {
            let Some(kit) = meta.kit.as_ref().filter(|k| k.is_shared()) else {
                return Err("This document is not a kit: `make_kit` first".to_owned());
            };
            let changes = ondin_core::kit::changes(&s.doc);
            if kit.version > 0 && !changes.any() {
                return Err(format!(
                    "Nothing changed since v{}: there is nothing to publish",
                    kit.version
                ));
            }
            let carried = crate::kits::publish::carried_names(&s.doc);
            let note = call
                .args
                .get("note")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let version = ondin_core::kit::publish(&mut s.doc, note, crate::library::clock::now())
                .ok_or("Nothing here to publish")?;
            (
                crate::kits::publish::published_said(&name, version, Some(&changes), &carried),
                json!({ "version": version }),
            )
        }
    };
    s.mark_unsaved();
    docs.save(path)?;
    docs.tell(format!("{said}{}", by(&call.client)));
    Ok(Answer::Json(answer))
}

#[cfg(test)]
mod tests {
    use super::super::serve::Headless;
    use super::super::{Documents, Engine};
    use ondin_mcp::queue::Call;
    use ondin_mcp::wire::{Content, Output};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    fn send(engine: &mut Engine, docs: &mut Headless, tool: &str, args: Value) -> Output {
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        engine.handle(
            docs,
            Call {
                conn: 1,
                client: "test".into(),
                tool: tool.into(),
                args,
                reply,
            },
        )
    }

    fn ok(engine: &mut Engine, docs: &mut Headless, tool: &str, args: Value) -> Value {
        let out = send(engine, docs, tool, args);
        assert!(!out.is_error, "{tool}: {:?}", out.content);
        out.structured.unwrap()
    }

    /// The refusal's text.
    fn refused(engine: &mut Engine, docs: &mut Headless, tool: &str, args: Value) -> String {
        let out = send(engine, docs, tool, args);
        assert!(out.is_error, "{tool} was not refused: {:?}", out.structured);
        match out.content.first() {
            Some(Content::Text(t)) => t.clone(),
            other => panic!("{tool}: {other:?}"),
        }
    }

    /// A `serve` host on a fresh library, holding Atlas — a kit whose one
    /// component's fill reads a colour variable in two modes, *Light* and *Dark*,
    /// published at v1 — and Consumer, with a mode of its own, *Night*, and an
    /// instance of that component. Every step is a tool call.
    struct Fixture {
        root: PathBuf,
        engine: Engine,
        docs: Headless,
        atlas: PathBuf,
        consumer: PathBuf,
        kit: String,
        instance: String,
        night: String,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let root = crate::kits::picker::tests::temp_root(tag);
        let mut engine = Engine::default();
        let mut docs = Headless::new(Some(root.clone()));
        let (e, d) = (&mut engine, &mut docs);
        let path = |v: &Value| PathBuf::from(v["path"].as_str().unwrap());

        let atlas = path(&ok(e, d, "create_document", json!({ "name": "Atlas" })));
        let page = ok(e, d, "use_document", json!({ "document": atlas }))["pages"][0]["id"].clone();
        ok(e, d, "add_mode", json!({ "name": "Light" }));
        ok(e, d, "add_mode", json!({ "name": "Dark" }));
        let ink = ok(
            e,
            d,
            "add_variable",
            json!({ "kind": "color", "name": "ink" }),
        )["made"]
            .clone();
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let rect = ok(
            e,
            d,
            "create_node",
            json!({ "parent": page, "kind": kind, "name": "Badge" }),
        )["created"][0]
            .clone();
        let fills = ondin_core::verbs::example("[Fill]").unwrap();
        ok(e, d, "set_fills", json!({ "node": rect, "fills": fills }));
        ok(
            e,
            d,
            "bind_variable",
            json!({ "nodes": [rect], "field": { "fill": 0 }, "variable": ink }),
        );
        let main = ok(e, d, "make_component", json!({ "nodes": [rect] }))["created"][0].clone();
        let kit = ok(e, d, "make_kit", json!({}))["id"]
            .as_str()
            .unwrap()
            .to_owned();
        ok(e, d, "publish_kit", json!({ "note": "First" }));

        let consumer = path(&ok(e, d, "create_document", json!({ "name": "Consumer" })));
        let page =
            ok(e, d, "use_document", json!({ "document": consumer }))["pages"][0]["id"].clone();
        let night = ok(e, d, "add_mode", json!({ "name": "Night" }))["made"]
            .as_str()
            .unwrap()
            .to_owned();
        let instance = ok(
            e,
            d,
            "insert_from_kit",
            json!({ "kit": "Atlas", "main": main, "parent": page }),
        )["created"][0]
            .as_str()
            .unwrap()
            .to_owned();
        Fixture {
            root,
            engine,
            docs,
            atlas,
            consumer,
            kit,
            instance,
            night,
        }
    }

    fn on_disk(path: &PathBuf) -> ondin_core::Document {
        ondin_core::io::load(&std::fs::read(path).unwrap()).unwrap()
    }

    /// `serve` answers Phase 4 from its own library: it lists what it filed,
    /// writes the kit's mark and publish and the consumer's insert to their
    /// files at once, and finds nothing waiting until the kit publishes again.
    ///
    /// flip: `Headless::with_library` lending no library → fails at
    /// `insert_from_kit` in the fixture, "There is no library folder".
    #[test]
    fn serve_files_publishes_and_inserts_through_its_own_library() {
        let mut f = fixture("serve");
        let (e, d) = (&mut f.engine, &mut f.docs);
        let listed = ok(e, d, "list_documents", json!({}));
        let names: Vec<&str> = listed["documents"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x["name"].as_str())
            .collect();
        assert!(
            names.contains(&"Atlas") && names.contains(&"Consumer"),
            "{names:?}"
        );

        let atlas = on_disk(&f.atlas);
        assert!(
            atlas
                .meta()
                .kit
                .as_ref()
                .is_some_and(|k| k.is_shared() && k.version == 1)
        );
        assert!(atlas.published().is_some(), "the publish is in the file");
        let consumer = on_disk(&f.consumer);
        let vault = consumer.vault().and_then(|v| consumer.get(v)).unwrap();
        assert_eq!(vault.children().len(), 1, "the insert is in the file");
        assert_eq!(
            ok(e, d, "list_kit_updates", json!({}))["updates"],
            json!([])
        );

        ok(e, d, "use_document", json!({ "document": f.atlas }));
        assert!(
            refused(e, d, "publish_kit", json!({})).contains("Nothing changed since v1"),
            "an unchanged kit has nothing to publish"
        );
        ok(e, d, "stop_sharing_kit", json!({}));
        assert!(
            on_disk(&f.atlas)
                .meta()
                .kit
                .as_ref()
                .is_some_and(|k| !k.is_shared())
        );
        assert_eq!(ok(e, d, "list_kits", json!({}))["kits"], json!([]));
    }

    /// `make_local` takes an instance to its kit component and reveals that as a
    /// local main on the page, out of the vault; a layer in no instance is
    /// refused by name.
    ///
    /// flip: `vault_top` returning the instance root rather than climbing to the
    /// main → fails at "no kit component".
    #[test]
    fn make_local_reveals_the_kit_component_an_instance_shows() {
        let mut f = fixture("make-local");
        let (e, d) = (&mut f.engine, &mut f.docs);
        let session = Documents::session(d, &f.consumer).unwrap();
        let page = session.doc.first_page();
        let page_wire = page.to_wire();
        let before = session.doc.get(page).unwrap().children().len();

        let why = refused(e, d, "make_local", json!({ "ids": [page_wire] }));
        assert!(why.contains("no kit component"), "{why}");

        let made = ok(e, d, "make_local", json!({ "ids": [f.instance] }));
        let top = made["made_local"][0].as_str().unwrap().to_owned();
        let s = Documents::session(d, &f.consumer).unwrap();
        let doc = &s.doc;
        let top = ondin_core::NodeId::from_wire(&top).unwrap();
        assert!(!doc.in_vault(top), "out of the vault");
        assert_eq!(
            doc.get(top).unwrap().parent(),
            Some(page),
            "a main on the page"
        );
        assert_eq!(doc.get(page).unwrap().children().len(), before + 1);
        let vault = doc.vault().and_then(|v| doc.get(v)).unwrap();
        assert!(vault.children().is_empty(), "nothing left vendored");
        assert!(
            on_disk(&f.consumer)
                .get(top)
                .is_some_and(|n| n.parent() == Some(page)),
            "written to the file"
        );
    }

    /// `map_kit_modes` maps the document's own mode onto a kit mode by id, `null`
    /// takes the mapping away, and a kit mode as the key or an unknown kit is
    /// refused, the record left as it was.
    ///
    /// flip: `map_modes` inserting into a clone it then drops → fails at the
    /// mapped mode.
    #[test]
    fn map_kit_modes_sets_and_clears_one_mapping() {
        let mut f = fixture("kit-modes");
        let (e, d) = (&mut f.engine, &mut f.docs);
        let night = serde_json::from_value::<ondin_core::variable::ModeId>(json!(f.night)).unwrap();
        let held = |d: &mut Headless| {
            Documents::session(d, &f.consumer)
                .unwrap()
                .doc
                .variables()
                .kits()
                .get(&f.kit)
                .cloned()
                .expect("the consumer holds Atlas's modes")
        };
        let modes = held(d);
        let dark = modes
            .modes
            .iter()
            .find(|m| m.name == "Dark")
            .expect("the kit's Dark mode")
            .id;
        let dark_wire = serde_json::to_value(dark).unwrap();

        ok(
            e,
            d,
            "map_kit_modes",
            json!({ "kit": "Atlas", "mapping": { (f.night.clone()): dark_wire } }),
        );
        assert_eq!(held(d).map.get(&night), Some(&dark), "the mapped mode");

        let why = refused(
            e,
            d,
            "map_kit_modes",
            json!({ "kit": "Atlas", "mapping": { (dark_wire.as_str().unwrap()): dark_wire } }),
        );
        assert!(why.contains("no mode of this document's own"), "{why}");
        let why = refused(
            e,
            d,
            "map_kit_modes",
            json!({ "kit": "Nope", "mapping": { (f.night.clone()): null } }),
        );
        assert!(why.contains("holds no modes of a kit"), "{why}");
        assert_eq!(
            held(d).map.get(&night),
            Some(&dark),
            "refusals change nothing"
        );

        ok(
            e,
            d,
            "map_kit_modes",
            json!({ "kit": f.kit, "mapping": { (f.night.clone()): null } }),
        );
        assert_eq!(held(d).map.get(&night), None, "null takes it away");
    }
}
