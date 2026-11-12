//! The window as an MCP host (§8.1): its endpoint, the queue drained at the top
//! of each frame, and its tabs lent to the engine.
//!
//! A document an agent asks for that no tab holds opens as a tab behind the one
//! in front. An agent's edit to the tab in front waits for the person's save or
//! the autosave, as the person's own edits do; one to a tab behind is written at
//! once, since a tab behind is kept clean.

use super::endpoint::{self, Endpoint, Host};
use super::{Documents, Engine};
use crate::app::OndinApp;
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
    let endpoint = match endpoint::listen(Host::Window, to) {
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
        engine: Engine::default(),
        _endpoint: endpoint,
    })
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
                Request::Call(call) => {
                    let reply = call.reply.clone();
                    let _ = reply.send(host.engine.handle(self, call));
                    answered = true;
                }
                Request::Closed(conn) => host.engine.closed(conn),
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

    fn committed(&mut self, path: &Path) {
        let Some(id) = self.tab_of_path(path) else {
            return;
        };
        if self.tabs.active == Some(id) {
            return;
        }
        let Some(Body::Parked(p)) = self.tabs.get_mut(id).map(|t| &mut t.body) else {
            return;
        };
        match crate::library::store::write_document_digest(path, &p.session.doc) {
            Ok(digest) => {
                p.session.disk = crate::library::disk::Stamp::of(path, digest);
                p.session.mark_saved(path.to_path_buf());
            }
            Err(e) => log::warn!(target: "ondin", "mcp: cannot write {}: {e}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_mcp::queue::Call;
    use serde_json::{Value, json};

    fn call(engine: &mut Engine, app: &mut OndinApp, tool: &str, args: Value) -> Value {
        let (reply, _answer) = std::sync::mpsc::sync_channel(1);
        let call = Call {
            conn: 1,
            client: "test".into(),
            tool: tool.into(),
            args,
            reply,
        };
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
}
