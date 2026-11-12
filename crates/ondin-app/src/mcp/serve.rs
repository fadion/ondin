//! `ondin serve` (§8.2): documents with no window, behind the MCP endpoint.
//!
//! Each document is an `EditorSession`, so a write takes the same commit door as
//! in the window; each commit is written to its file at once, since nobody is
//! there to save later. The process ends when a window starts, which takes over,
//! and after its last connection closes.

use super::endpoint::{self, Host};
use super::{Documents, Engine};
use crate::library::state::Library;
use crate::session::EditorSession;
use ondin_mcp::queue::{self, Request};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// How long a `serve` nobody has connected to waits for its first client.
const FIRST_WAIT: Duration = Duration::from_secs(60);

/// How often it looks for a window to hand over to.
const WINDOW_EVERY: Duration = Duration::from_secs(2);

/// The headless host's documents.
pub(crate) struct Headless {
    open: HashMap<PathBuf, EditorSession>,
    library: Option<PathBuf>,
    /// The library, scanned the first time a kit tool asks for it.
    scanned: Option<Library>,
}

impl Headless {
    pub(crate) fn new(library: Option<PathBuf>) -> Self {
        Self {
            open: HashMap::new(),
            library,
            scanned: None,
        }
    }

    /// The library, scanned again.
    fn rescan(&mut self) {
        match (&mut self.scanned, &self.library) {
            (Some(lib), _) => lib.refresh(),
            (None, Some(root)) => self.scanned = Some(Library::open(root.clone())),
            (None, None) => {}
        }
    }
}

impl Documents for Headless {
    fn open(&mut self, wanted: &str) -> Result<PathBuf, String> {
        let open: Vec<PathBuf> = self.open.keys().cloned().collect();
        let path = super::resolve(wanted, &open, self.library.as_deref())?;
        if !self.open.contains_key(&path) {
            let bytes =
                std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let doc = ondin_core::io::load(&bytes)
                .map_err(|e| format!("cannot load {}: {e}", path.display()))?;
            let mut session = EditorSession::new();
            session.adopt_document(doc, Some(path.clone()));
            self.open.insert(path.clone(), session);
        }
        Ok(path)
    }

    fn session(&mut self, path: &Path) -> Option<&mut EditorSession> {
        self.open.get_mut(path)
    }

    fn committed(&mut self, path: &Path) {
        if let Err(e) = self.save(path) {
            log::warn!(target: "ondin", "serve: {e}");
        }
    }

    fn library(&mut self) -> Option<&Library> {
        self.rescan();
        self.scanned.as_ref()
    }

    fn with_library(&mut self, path: &Path) -> Option<(&mut EditorSession, Option<&Library>)> {
        self.rescan();
        Some((self.open.get_mut(path)?, self.scanned.as_ref()))
    }

    fn save(&mut self, path: &Path) -> Result<(), String> {
        let session = self
            .open
            .get_mut(path)
            .ok_or_else(|| format!("{} is no longer open", path.display()))?;
        crate::library::store::write_document(path, &session.doc)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        session.mark_saved(path.to_path_buf());
        Ok(())
    }

    /// Nobody reads a status line here; the log keeps it.
    fn tell(&mut self, line: String) {
        log::info!(target: "ondin", "serve: {line}");
    }
}

/// Run `ondin serve` until a window takes over or the last client leaves.
pub(crate) fn run() -> Result<(), String> {
    let (to, from) = queue::queue();
    let _endpoint = endpoint::listen(Host::Serve, to)
        .map_err(|e| format!("ondin serve: cannot listen: {e}"))?;
    let prefs = crate::prefs::Prefs::load();
    let mut docs = Headless::new(crate::library::root(prefs.base_folder.as_deref()));
    let mut engine = Engine::default();
    let started = Instant::now();
    let mut looked = Instant::now();
    let mut served = false;
    loop {
        match from.recv_timeout(Duration::from_millis(500)) {
            Ok(Request::Call(call)) => {
                served = true;
                let reply = call.reply.clone();
                let _ = reply.send(engine.handle(&mut docs, call));
            }
            Ok(Request::Closed(conn)) => {
                engine.closed(conn);
                let used: Vec<PathBuf> = engine.in_use().map(Path::to_path_buf).collect();
                docs.open.retain(|p, _| used.contains(p));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        if !engine.has_connections() && (served || started.elapsed() > FIRST_WAIT) {
            return Ok(());
        }
        if looked.elapsed() > WINDOW_EVERY {
            looked = Instant::now();
            if endpoint::hosts().iter().any(|h| h.host == Host::Window) {
                return Ok(());
            }
        }
    }
}
