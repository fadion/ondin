//! The window as an MCP host (§8.1): its endpoint, the queue drained at the top
//! of each frame, and its tabs lent to the engine.
//!
//! A document an agent asks for that no tab holds opens as a tab behind the one
//! in front. An agent's edit to the tab in front waits for the person's save or
//! the autosave, as the person's own edits do; one to a tab behind is written at
//! once, since a tab behind is kept clean.

use super::access::Grants;
use super::endpoint::{self, Endpoint, Host};
use super::propose::Proposal;
use super::{Documents, Engine};
use crate::app::OndinApp;
use crate::library::state::Library;
use crate::session::EditorSession;
use crate::tabs::{Body, Parked};
use ondin_mcp::queue::{self, Request};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

/// The window's endpoint and the engine answering it.
pub(crate) struct WindowHost {
    calls: Receiver<Request>,
    engine: Engine,
    _endpoint: Endpoint,
}

/// Listen for agents. A relay thread hands each call on and wakes the window,
/// which otherwise sleeps until the person moves. `None`, logged, when the
/// endpoint cannot be made: the window works without it.
pub(crate) fn start(ctx: &egui::Context) -> Option<WindowHost> {
    let (to, from) = queue::queue();
    let engine = Engine::default();
    let lists = engine.grants().lists();
    let endpoint = match endpoint::listen(Host::Window, to, Some(lists)) {
        Ok(e) => e,
        Err(e) => {
            log::warn!(target: "ondin", "mcp: no endpoint: {e}");
            return None;
        }
    };
    let (relay, calls) = channel();
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("mcp-relay".into())
        .spawn(move || {
            for request in from {
                if relay.send(request).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        log::warn!(target: "ondin", "mcp: no relay: {e}");
        return None;
    }
    Some(WindowHost {
        calls,
        engine,
        _endpoint: endpoint,
    })
}

impl WindowHost {
    /// The levels granted to the assistant's chats; a grant made here holds for
    /// the next call and the next `tools/list`.
    pub(crate) fn grants(&self) -> Grants {
        self.engine.grants()
    }

    /// The proposal called `id`, while the host keeps it.
    pub(crate) fn proposal(&self, id: u64) -> Option<&Proposal> {
        self.engine.proposal(id)
    }
}

impl OndinApp {
    /// Answer every call waiting, before this frame reads input.
    pub(crate) fn drain_mcp(&mut self) {
        let Some(mut host) = self.mcp.take() else {
            return;
        };
        let mut answered = false;
        while let Ok(request) = host.calls.try_recv() {
            match request {
                Request::Call(call) if call.take() => {
                    let reply = call.reply.clone();
                    let _ = reply.send(host.engine.handle(self, call));
                    answered = true;
                }
                // Its connection gave up waiting and told the agent nothing changed.
                Request::Call(_) => {}
                Request::Closed(conn) => {
                    host.engine.closed(conn);
                }
            }
        }
        self.mcp = Some(host);
        if answered {
            self.egui_ctx.request_repaint();
        }
    }

    /// Read `path` into a tab behind the one in front.
    fn open_behind(&mut self, path: &Path) -> Result<(), String> {
        let (doc, history, stamp) =
            crate::app::read_opened(path, &self.library.root, self.prefs.history_size > 0)?;
        if let Some(id) = doc.meta().id.clone() {
            self.library.local.mark_opened(&id);
            self.library.local.save();
        }
        let mut session = EditorSession::new();
        session.adopt_document(doc, Some(path.to_path_buf()));
        session.disk = stamp;
        if let Some(history) = history {
            session.resume_history(history);
        }
        let parked = Box::new(Parked::opened(session));
        match self.tab_of_file(path) {
            Some(id) => {
                if let Some(tab) = self.tabs.get_mut(id) {
                    tab.body = Body::Parked(parked);
                }
            }
            None => {
                self.tabs.add(Body::Parked(parked));
            }
        }
        Ok(())
    }
}

impl Documents for OndinApp {
    fn open(&mut self, wanted: &str) -> Result<PathBuf, String> {
        let open: Vec<PathBuf> = self
            .tabs
            .list
            .iter()
            .filter_map(|t| self.tab_path(t).map(Path::to_path_buf))
            .collect();
        let path = super::resolve(wanted, &open, Some(&self.library.root))?;
        let unloaded = self
            .tab_of_file(&path)
            .and_then(|id| self.tabs.get(id))
            .is_none_or(|t| matches!(t.body, Body::Unloaded { .. }));
        if unloaded {
            self.open_behind(&path)?;
        }
        // The tab's own spelling, which `session` looks up without the disk.
        let id = self.tab_of_file(&path).ok_or("the tab did not open")?;
        let tab = self.tabs.get(id).ok_or("the tab did not open")?;
        Ok(self.tab_path(tab).unwrap_or(&path).to_path_buf())
    }

    fn session(&mut self, path: &Path) -> Option<&mut EditorSession> {
        let id = self.tab_of_path(path)?;
        if self.tabs.active == Some(id) {
            return Some(&mut self.session);
        }
        match &mut self.tabs.get_mut(id)?.body {
            Body::Parked(p) => Some(&mut p.session),
            _ => None,
        }
    }

    fn committed(&mut self, path: &Path) -> Result<(), String> {
        let behind = self
            .tab_of_path(path)
            .is_some_and(|id| self.tabs.active != Some(id));
        if behind {
            return self.write_behind(path);
        }
        // A family an agent named loads as one the person picked does; a tab
        // behind loads its families when it comes to the front.
        self.ensure_document_fonts();
        Ok(())
    }

    fn fonts(&mut self) -> Vec<String> {
        let all = self.fonts.match_count("");
        self.fonts.matched_names("", 0..all)
    }

    fn canvas(&mut self, path: &Path) -> Option<(u32, u32)> {
        let id = self.tab_of_path(path)?;
        (self.tabs.active == Some(id)).then_some(self.canvas_px)
    }

    fn library(&mut self) -> Option<&Library> {
        self.library.refresh();
        Some(&self.library)
    }

    fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)> {
        self.library.refresh();
        let id = self.tab_of_path(path)?;
        let session = match self.tabs.active == Some(id) {
            true => &mut self.session,
            false => match &mut self.tabs.get_mut(id)?.body {
                Body::Parked(p) => &mut p.session,
                _ => return None,
            },
        };
        Some((session, Some(&self.library)))
    }

    fn save(&mut self, path: &Path) -> Result<(), String> {
        let id = self
            .tab_of_path(path)
            .ok_or_else(|| format!("{} is no longer open", path.display()))?;
        if self.tabs.active != Some(id) {
            return self.write_behind(path);
        }
        match self.save_file(false) {
            true => Ok(()),
            false => Err(format!(
                "{} could not be saved, and the change waits in the open tab for the \
                 person's next save; the window's status line says why",
                path.display()
            )),
        }
    }

    fn tell(&mut self, line: String) {
        self.session.info(line);
    }

    fn before_write(&mut self, path: &Path) {
        if self.tab_of_path(path).is_some() && self.tab_of_path(path) == self.tabs.active {
            self.finish_text_first();
        }
    }
}

impl OndinApp {
    /// Write the tab behind holding `path` to its file, as a tab behind is
    /// kept clean.
    fn write_behind(&mut self, path: &Path) -> Result<(), String> {
        let id = self.tab_of_path(path).ok_or("the tab closed")?;
        let Some(Body::Parked(p)) = self.tabs.get_mut(id).map(|t| &mut t.body) else {
            return Err(format!("{} is not loaded", path.display()));
        };
        // A change made outside since the tab was read is not overwritten: the
        // tab keeps the edit unsaved and asks the person when it comes to the front.
        if p.session.disk.as_mut().is_some_and(|s| s.check(path)) {
            p.session.clash = crate::session::Clash::Ask;
            return Err(format!(
                "{} changed on disk since Ondin read it; the change waits in its tab and \
                 the person decides which to keep",
                path.display()
            ));
        }
        let digest = crate::library::store::write_document_digest(path, &p.session.doc)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        p.session.disk = crate::library::disk::Stamp::of(path, digest);
        p.session.mark_saved(path.to_path_buf());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_mcp::queue::Call;
    use serde_json::{Value, json};

    fn call(engine: &mut Engine, app: &mut OndinApp, tool: &str, args: Value) -> Value {
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        let call = Call::new(1, None, tool, args, reply);
        let out = engine.handle(app, call);
        assert!(!out.is_error, "{tool}: {:?}", out.content);
        out.structured.unwrap()
    }

    /// An agent's document opens as a tab behind, the tab in front keeps the
    /// fields, and an edit to the tab behind is written to its file at once.
    ///
    /// flip: `committed` returning before the write → fails at "written".
    #[test]
    fn an_agents_document_opens_behind_and_its_edits_are_written() {
        let dir = std::env::temp_dir().join(format!("ondin-mcp-window-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dashboard.ondin");
        let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
        let doc = ondin_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        std::fs::write(&file, ondin_core::io::save(&doc).unwrap()).unwrap();
        let before = std::fs::read(&file).unwrap();

        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let front = app.tabs.active;
        let tabs = app.tabs.list.len();
        let mut engine = Engine::default();
        let opened = call(
            &mut engine,
            &mut app,
            "use_document",
            json!({ "document": file.display().to_string() }),
        );
        assert_eq!(opened["pages"][0]["id"], json!(page));
        assert_eq!(app.tabs.list.len(), tabs + 1, "a tab opened");
        assert_eq!(app.tabs.active, front, "the tab in front stayed");

        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        call(
            &mut engine,
            &mut app,
            "create_node",
            json!({ "parent": page, "kind": kind, "name": "Chart" }),
        );
        assert_ne!(std::fs::read(&file).unwrap(), before, "written");
        let id = app.tab_of_path(&file).unwrap();
        let Some(Body::Parked(p)) = app.tabs.get(id).map(|t| &t.body) else {
            panic!("not parked");
        };
        assert!(!p.session.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An agent's write to the tab in front ends the person's text edit first,
    /// so what they typed is kept as its own step; a read leaves it open.
    ///
    /// flip: `OndinApp::before_write` doing nothing → fails at "the edit ended".
    #[test]
    fn an_agents_write_keeps_the_persons_typing() {
        let dir = std::env::temp_dir().join(format!("ondin-mcp-typing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dashboard.ondin");
        let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
        let doc = ondin_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        std::fs::write(&file, ondin_core::io::save(&doc).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut engine = Engine::default();
        let document = json!({ "document": file.display().to_string() });
        call(&mut engine, &mut app, "use_document", document);
        app.activate_tab(app.tab_of_path(&file).unwrap());
        let made = call(
            &mut engine,
            &mut app,
            "create_text",
            json!({ "parent": page, "text": "Hello" }),
        );
        let id = ondin_core::NodeId::from_wire(made["created"][0].as_str().unwrap()).unwrap();
        let node = app.session.doc.get(id).unwrap();
        let parts = ondin_core::TextParts::of(node.kind()).unwrap();
        let original = (
            "Hello".to_owned(),
            parts.spans.clone(),
            parts.para_spans.clone(),
        );
        app.text = Some(crate::preview::TextSession {
            id,
            editor: ondin_core::TextEdit::new("Hello there", parts),
            original,
            created: false,
            blink: Default::default(),
        });
        app.mode = crate::input::Mode::TextInsert;
        call(
            &mut engine,
            &mut app,
            "get_node",
            json!({ "id": id.to_wire() }),
        );
        assert!(app.text.is_some(), "a read leaves the edit open");
        call(
            &mut engine,
            &mut app,
            "set_name",
            json!({ "node": page, "name": "P" }),
        );
        assert!(app.text.is_none(), "the edit ended");
        let ondin_core::NodeKind::Text { content, .. } = app.session.doc.get(id).unwrap().kind()
        else {
            panic!("not a text");
        };
        assert_eq!(content.as_str(), "Hello there", "typing kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// While the person compares versions, an agent's `undo` and `redo` are
    /// refused as read-only and change nothing.
    ///
    /// flip: `run` without its compare arm for undo/redo → fails at "refused".
    #[test]
    fn an_agents_undo_is_refused_while_comparing() {
        let dir = std::env::temp_dir().join(format!("ondin-mcp-compare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("dashboard.ondin");
        let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
        let doc = ondin_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        std::fs::write(&file, ondin_core::io::save(&doc).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let mut engine = Engine::default();
        let document = json!({ "document": file.display().to_string() });
        call(&mut engine, &mut app, "use_document", document);
        let id = app.tab_of_path(&file).unwrap();
        app.activate_tab(id);
        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let args = json!({ "parent": page, "kind": kind, "name": "Chart" });
        call(&mut engine, &mut app, "create_node", args);
        let pick = crate::compare::Pick {
            path: file.clone(),
            title: "Before".into(),
            when: None,
        };
        app.compare_with(pick, None);
        assert!(app.session.compare.is_some(), "the compare opened");
        let count = app.session.doc.len();
        for verb in ["undo", "redo"] {
            let (reply, _answer) = std::sync::mpsc::sync_channel(1);
            let out = engine.handle(&mut app, Call::new(1, None, verb, json!({}), reply));
            assert!(out.is_error, "{verb} refused");
        }
        assert_eq!(app.session.doc.len(), count, "nothing changed");
        app.leave_compare();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A kit made and published by an agent from a tab behind, used by another
    /// document behind through `get_kit` and `insert_from_kit`, then published
    /// again and taken with `accept_kit_updates` — each saved where it must be,
    /// and the publish said on the status line with the client's name.
    ///
    /// flip: `waiting` keeping kits at the held version (`>` to `>=`) → fails at
    /// the empty update list right after the insert.
    #[test]
    fn an_agent_makes_publishes_uses_and_updates_a_kit() {
        use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
        use ondin_core::{NodeKind, Operation, Transaction};
        let root = crate::kits::picker::tests::temp_root("mcp-phase4");
        let ctx = egui::Context::default();
        let mut app = crate::kits::picker::tests::app_on(&ctx, root.clone());
        let front = app.tabs.active;
        let mut engine = Engine::default();
        let mut go =
            |app: &mut OndinApp, tool: &str, args: Value| call(&mut engine, app, tool, args);

        // Atlas holds one main, Badge; Consumer is empty.
        let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
        let mut atlas = ondin_core::Document::new(ids.mint());
        let badge = ids.mint();
        atlas
            .apply(&Transaction(vec![
                Operation::CreateNode {
                    id: badge,
                    parent: atlas.first_page(),
                    index: 0,
                    kind: NodeKind::Artboard {
                        size: Size::new(40.0, 20.0),
                        corner_radii: RoundedRectRadii::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: Some(Affine::IDENTITY),
                    name: Some("Badge".into()),
                },
                Operation::SetComponent {
                    id: badge,
                    component: true,
                },
            ]))
            .unwrap();
        let atlas_path =
            crate::library::store::file_document(&root, None, "Atlas", &mut atlas).unwrap();
        let made = go(&mut app, "create_document", json!({ "name": "Consumer" }));
        let consumer = made["path"].as_str().unwrap().to_owned();
        assert!(
            app.session
                .status()
                .text
                .contains("Filed Consumer · by test"),
            "{}",
            app.session.status().text
        );

        go(
            &mut app,
            "use_document",
            json!({ "document": atlas_path.display().to_string() }),
        );
        go(&mut app, "make_kit", json!({}));
        let published = go(&mut app, "publish_kit", json!({ "note": "First" }));
        assert_eq!(published["version"], json!(1));
        assert!(
            app.session.status().text.ends_with(" · by test"),
            "{}",
            app.session.status().text
        );
        let on_disk = ondin_core::io::load(&std::fs::read(&atlas_path).unwrap()).unwrap();
        assert!(on_disk.published().is_some(), "the publish was saved");
        let kits = go(&mut app, "list_kits", json!({}));
        assert_eq!(kits["kits"][0]["name"], json!("Atlas"));

        go(&mut app, "use_document", json!({ "document": consumer }));
        let kit = go(&mut app, "get_kit", json!({ "kit": "atlas" }));
        assert_eq!(kit["components"][0]["id"], json!(badge.to_wire()));
        let consumer_page = {
            let path = std::path::PathBuf::from(&consumer);
            let session = Documents::session(&mut app, &path).unwrap();
            session.doc.first_page().to_wire()
        };
        let placed = go(
            &mut app,
            "insert_from_kit",
            json!({ "kit": "Atlas", "main": badge.to_wire(), "parent": consumer_page }),
        );
        assert_eq!(placed["created"].as_array().map(Vec::len), Some(1));
        assert_eq!(
            go(&mut app, "list_kit_updates", json!({}))["updates"],
            json!([])
        );

        go(
            &mut app,
            "use_document",
            json!({ "document": atlas_path.display().to_string() }),
        );
        go(
            &mut app,
            "set_name",
            json!({ "node": badge.to_wire(), "name": "Pill" }),
        );
        assert_eq!(go(&mut app, "publish_kit", json!({}))["version"], json!(2));

        go(&mut app, "use_document", json!({ "document": consumer }));
        let waiting = go(&mut app, "list_kit_updates", json!({}));
        assert_eq!(waiting["updates"][0]["held"], json!(1));
        assert_eq!(waiting["updates"][0]["version"], json!(2));
        let accepted = go(&mut app, "accept_kit_updates", json!({}));
        assert_eq!(accepted["accepted"][0]["version"], json!(2));
        assert_eq!(
            go(&mut app, "list_kit_updates", json!({}))["updates"],
            json!([]),
            "nothing waits after the accept"
        );
        let written = ondin_core::io::load(&std::fs::read(&consumer).unwrap()).unwrap();
        assert!(
            written.vault().is_some(),
            "the consumer's file holds the kit's main"
        );
        assert_eq!(app.tabs.active, front, "the tab in front never changed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// On the tab in front, an agent's edit waits for the person's save, while
    /// `make_kit` saves at once through that save — the waiting edit with it, as
    /// the app's own *Make this a kit* does — and a save the disk refuses (the
    /// file changed under the tab) fails the tool and leaves the file alone.
    ///
    /// flip: `Documents::save` answering `Ok` for the front tab without saving →
    /// fails at "the mark is in the file".
    #[test]
    fn a_kit_mark_on_the_front_tab_saves_through_the_persons_save() {
        let root = crate::kits::picker::tests::temp_root("mcp-front-save");
        let ctx = egui::Context::default();
        let mut app = crate::kits::picker::tests::app_on(&ctx, root.clone());
        let front = app.tabs.active;
        let file = app.session.path.clone().expect("the front tab is filed");
        let mut engine = Engine::default();
        let opened = call(
            &mut engine,
            &mut app,
            "use_document",
            json!({ "document": file.display().to_string() }),
        );
        assert_eq!(
            app.tabs.active, front,
            "the agent's document is the front tab"
        );
        let read = |file: &Path| ondin_core::io::load(&std::fs::read(file).unwrap()).unwrap();

        let kind = ondin_core::verbs::example("NodeKind").unwrap();
        let made = call(
            &mut engine,
            &mut app,
            "create_node",
            json!({ "parent": opened["pages"][0]["id"], "kind": kind, "name": "Card" }),
        );
        let card = ondin_core::NodeId::from_wire(made["created"][0].as_str().unwrap()).unwrap();
        assert!(
            read(&file).get(card).is_none(),
            "an edit waits for the save"
        );
        assert!(app.session.is_dirty());

        call(&mut engine, &mut app, "make_kit", json!({}));
        let saved = read(&file);
        assert!(
            saved.meta().kit.as_ref().is_some_and(|k| k.is_shared()),
            "the mark is in the file"
        );
        assert!(saved.get(card).is_some(), "the waiting edit went with it");
        assert!(!app.session.is_dirty());
        assert!(
            app.session.status().text.ends_with(" · by test"),
            "{}",
            app.session.status().text
        );

        // Another writer changes the file under the tab.
        let mut theirs = saved.clone();
        let mut meta = theirs.meta().clone();
        meta.name = Some("Changed elsewhere".into());
        theirs.set_meta(meta);
        std::fs::write(&file, ondin_core::io::save(&theirs).unwrap()).unwrap();
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        let out = engine.handle(
            &mut app,
            Call::new(1, None, "stop_sharing_kit", json!({}), reply),
        );
        assert!(
            out.is_error
                && matches!(out.content.first(), Some(ondin_mcp::wire::Content::Text(t))
                    if t.contains("could not be saved")),
            "the refused save fails the tool: {:?}",
            out.content
        );
        assert_eq!(
            app.session.clash,
            crate::session::Clash::Ask,
            "the person is asked about the changed file"
        );
        assert_eq!(
            read(&file).meta().name.as_deref(),
            Some("Changed elsewhere"),
            "the other writer's file is left alone"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
