//! `ondin serve` (§8.2): documents with no window, behind the MCP endpoint.
//!
//! Each document is an `EditorSession`, so a write takes the same commit door as
//! in the window; each commit is written to its file at once, since nobody is
//! there to save later. The process ends when a window starts, which takes over,
//! and after its last connection closes — [`KEYED_LINGER`] after, while it
//! remembers a keyed client.

use super::endpoint::{self, Host};
use super::system_fonts::SystemFonts;
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

/// How long, after its last answer, it waits before handing over to a window.
const FLUSH_WAIT: Duration = Duration::from_secs(1);

/// How long it outlives its last connection while it remembers a keyed client:
/// a harness that starts a process per turn reconnects at the next turn, and
/// finds its document and revisions where it left them.
const KEYED_LINGER: Duration = Duration::from_secs(15 * 60);

/// The headless host's documents.
pub(crate) struct Headless {
    open: HashMap<PathBuf, EditorSession>,
    library: Option<PathBuf>,
    /// The library, scanned the first time a kit tool asks for it.
    scanned: Option<Library>,
    /// The installed fonts, read the first time a document or `list_fonts`
    /// needs them.
    fonts: Option<SystemFonts>,
}

impl Headless {
    pub(crate) fn new(library: Option<PathBuf>) -> Self {
        Self {
            open: HashMap::new(),
            library,
            scanned: None,
            fonts: None,
        }
    }

    /// Register the installed families `path`'s document names, and lay its
    /// text out again when any landed: `serve`'s engine starts with Inter alone.
    fn load_fonts(&mut self, path: &Path) {
        let Some(session) = self.open.get_mut(path) else {
            return;
        };
        let fonts = self.fonts.get_or_insert_with(SystemFonts::load);
        if fonts.register_for(&session.doc) {
            session.fonts_changed();
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
        // The open document's own spelling for the same file (a case difference on
        // Windows, a `..`), so one file never gets two sessions writing over each
        // other.
        if !self.open.contains_key(&path)
            && let Ok(want) = std::fs::canonicalize(&path)
            && let Some(same) = open
                .iter()
                .find(|p| std::fs::canonicalize(p).is_ok_and(|p| p == want))
        {
            return Ok(same.clone());
        }
        if !self.open.contains_key(&path) {
            let read = |e: std::io::Error| format!("cannot read {}: {e}", path.display());
            // Looked at before the bytes are read, so a write in between shows.
            let meta = std::fs::metadata(&path).map_err(read)?;
            let bytes = std::fs::read(&path).map_err(read)?;
            let doc = ondin_core::io::load(&bytes)
                .map_err(|e| format!("cannot load {}: {e}", path.display()))?;
            let mut session = EditorSession::new();
            session.adopt_document(doc, Some(path.clone()));
            session.disk = Some(crate::library::disk::Stamp::from_meta(
                &meta,
                crate::library::history::digest(&bytes),
            ));
            self.open.insert(path.clone(), session);
            self.load_fonts(&path);
        }
        Ok(path)
    }

    fn session(&mut self, path: &Path) -> Option<&mut EditorSession> {
        self.open.get_mut(path)
    }

    fn committed(&mut self, path: &Path) -> Result<(), String> {
        self.load_fonts(path);
        self.save(path)
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
        // A change made outside since `serve` read the file (a pull, a sync, a
        // merge) wins: nobody is here to choose, so this copy is dropped unwritten
        // and the next `use_document` reads the file as it is.
        if session.disk.as_mut().is_some_and(|s| s.check(path)) {
            self.open.remove(path);
            return Err(format!(
                "{} changed on disk since Ondin read it, so this edit was not written; \
                 call `use_document` to read it again",
                path.display()
            ));
        }
        let digest = crate::library::store::write_document_digest(path, &session.doc)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        session.disk = crate::library::disk::Stamp::of(path, digest);
        session.mark_saved(path.to_path_buf());
        Ok(())
    }

    /// Nobody reads a status line here; the log keeps it.
    fn tell(&mut self, line: String) {
        log::info!(target: "ondin", "serve: {line}");
    }

    /// The installed families: `serve` downloads no web font.
    fn fonts(&mut self) -> Vec<String> {
        self.fonts
            .get_or_insert_with(SystemFonts::load)
            .families()
            .to_vec()
    }

    fn canvas(&mut self, _: &Path) -> Option<(u32, u32)> {
        None
    }
}

/// When `serve` ends: its own count of the connections that called, so one whose
/// every call was refused before the engine knew it still counts, and a probe
/// that never called does not.
#[derive(Default)]
struct Life {
    callers: std::collections::HashSet<ondin_mcp::queue::Conn>,
    served: bool,
    /// When the last caller left, while none is open.
    idle: Option<Instant>,
    answered: Option<Instant>,
}

impl Life {
    fn called(&mut self, conn: ondin_mcp::queue::Conn, now: Instant) {
        self.callers.insert(conn);
        self.served = true;
        self.idle = None;
        self.answered = Some(now);
    }

    fn closed(&mut self, conn: ondin_mcp::queue::Conn, now: Instant) {
        if self.callers.remove(&conn) && self.callers.is_empty() {
            self.idle = Some(now);
        }
    }

    /// Whether to end with nobody left: `linger` after the last caller, or
    /// [`FIRST_WAIT`] after `started` when nobody ever called.
    fn done(&self, started: Instant, linger: Duration, now: Instant) -> bool {
        if !self.callers.is_empty() {
            return false;
        }
        match self.idle {
            Some(since) => now.duration_since(since) >= linger,
            None => !self.served && now.duration_since(started) > FIRST_WAIT,
        }
    }

    /// Whether to hand over to a window: once the last answer has had time to
    /// reach its connection, since ending the process drops an unwritten reply.
    fn hand_over(&self, now: Instant) -> bool {
        self.callers.is_empty()
            || self
                .answered
                .is_none_or(|a| now.duration_since(a) > FLUSH_WAIT)
    }
}

/// The lock only one `serve` holds, `None` when another does. Without a run
/// folder, or a lock the system will not take, it runs unguarded.
fn one_serve() -> Option<Option<std::fs::File>> {
    let Some(dir) = endpoint::run_dir() else {
        return Some(None);
    };
    let file = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("serve.lock"))
    });
    let Ok(file) = file else {
        return Some(None);
    };
    match file.try_lock() {
        Ok(()) => Some(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => None,
        Err(std::fs::TryLockError::Error(_)) => Some(None),
    }
}

/// Run `ondin serve` until a window takes over or the last client leaves.
pub(crate) fn run() -> Result<(), String> {
    // One `serve` per user: two proxies starting together would otherwise each
    // start one, and two sessions of one file overwrite each other's writes.
    // The lock lasts as long as this process; a second `serve` leaves at once,
    // and its proxy connects to the first.
    let _only = match one_serve() {
        Some(lock) => lock,
        None => return Ok(()),
    };
    let (to, from) = queue::queue();
    let _endpoint = endpoint::listen(Host::Serve, to, None)
        .map_err(|e| format!("ondin serve: cannot listen: {e}"))?;
    let prefs = crate::prefs::Prefs::load();
    let mut docs = Headless::new(crate::library::root(prefs.base_folder.as_deref()));
    let mut engine = Engine::default();
    let started = Instant::now();
    let mut looked = Instant::now();
    let mut life = Life::default();
    let mut window = false;
    loop {
        match from.recv_timeout(Duration::from_millis(500)) {
            // One its connection gave up waiting for is not run.
            Ok(Request::Call(call)) if !call.take() => {}
            Ok(Request::Call(call)) => {
                let conn = call.conn;
                let reply = call.reply.clone();
                let _ = reply.send(engine.handle(&mut docs, call));
                life.called(conn, Instant::now());
            }
            // A connection that never called, such as a liveness probe from
            // `endpoint::hosts` (this process's own among them), is no client
            // leaving: counting it ended `serve` before its first call.
            Ok(Request::Closed(conn)) => {
                if engine.closed(conn) {
                    let used: Vec<PathBuf> = engine.in_use().map(Path::to_path_buf).collect();
                    docs.open.retain(|p, _| used.contains(p));
                }
                life.closed(conn, Instant::now());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        let linger = match engine.remembers_clients() {
            true => KEYED_LINGER,
            false => Duration::ZERO,
        };
        if life.done(started, linger, Instant::now()) {
            return Ok(());
        }
        if !window && looked.elapsed() > WINDOW_EVERY {
            looked = Instant::now();
            window = endpoint::hosts().iter().any(|h| h.host == Host::Window);
        }
        if window && life.hand_over(Instant::now()) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_mcp::queue::Call;
    use serde_json::json;

    /// A caller whose calls were all refused still ends `serve` when it leaves;
    /// a probe that never called does not; nobody calling ends it after the
    /// first wait.
    ///
    /// flip: `Life::closed` ignoring a conn the engine never knew → fails at
    /// "the refused caller left".
    #[test]
    fn serve_ends_when_its_last_caller_leaves() {
        let t0 = Instant::now();
        let mut life = Life::default();
        life.closed(7, t0);
        assert!(!life.done(t0, Duration::ZERO, t0), "a probe is no client");
        assert!(
            life.done(t0, Duration::ZERO, t0 + FIRST_WAIT * 2),
            "nobody came"
        );
        life.called(1, t0);
        assert!(!life.done(t0, Duration::ZERO, t0), "a caller is open");
        assert!(!life.hand_over(t0), "its answer has not been flushed");
        assert!(life.hand_over(t0 + FLUSH_WAIT * 2));
        life.closed(1, t0);
        assert!(life.done(t0, Duration::ZERO, t0), "the refused caller left");
        assert!(!life.done(t0, KEYED_LINGER, t0), "a keyed client lingers");
    }

    /// An agent's write through `serve` is not written over a file changed on
    /// disk since it was read: the outside change stays, and the call says so.
    ///
    /// flip: `Headless::save` without its stamp check → fails at "the clash is
    /// reported".
    #[test]
    fn serve_does_not_write_over_a_change_on_disk() {
        let dir = std::env::temp_dir().join(format!("ondin-serve-clash-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("brand.ondin");
        let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
        let doc = ondin_core::Document::new(ids.mint());
        let page = doc.first_page().to_wire();
        std::fs::write(&file, ondin_core::io::save(&doc).unwrap()).unwrap();
        let mut docs = Headless::new(None);
        let mut engine = Engine::default();
        let mut run = |tool: &str, args: serde_json::Value| {
            let (reply, _answer) = std::sync::mpsc::sync_channel(1);
            engine.handle(&mut docs, Call::new(1, None, tool, args, reply))
        };
        let path = file.display().to_string();
        assert!(!run("use_document", json!({ "document": path })).is_error);
        let rename = |name: &str| json!({ "node": page, "name": name });
        assert!(!run("set_name", rename("Ours")).is_error, "written");
        // Another program writes the file; a second's wait lets its time move.
        std::thread::sleep(Duration::from_millis(1100));
        let outside = ondin_core::io::save(&ondin_core::Document::new(ids.mint())).unwrap();
        std::fs::write(&file, &outside).unwrap();
        let out = run("set_name", rename("Mine"));
        let unsaved = out.structured.as_ref().and_then(|v| v.get("unsaved"));
        assert!(
            unsaved.is_some(),
            "the clash is reported: {:?}",
            out.content
        );
        assert_eq!(std::fs::read(&file).unwrap(), outside, "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
