//! How many documents use a kit — the count *Stop sharing* says (§5.3e; §15
//! D1021).
//!
//! **Read by loading every file, on a thread of its own** — the maintainer's
//! ruling over writing a consumer's kit ids into its prefix-readable `meta`. The
//! library scan reads each file's first 4 KB (`io::probe::meta_in_prefix`), and
//! which kits a document uses is in its vault, past that prefix; a `meta` field
//! would have been a schema addition that leaves every file saved before it
//! uncounted. A whole load per file is the cost — not measured here; the kit
//! shelf's worker exists for the same load (`kits::index`) — so it runs off the
//! frame, asked once when the dialog opens, and the number arrives while the
//! dialog is read.
//!
//! A document **uses** a kit when its vault holds a main that came from it,
//! `origin.kit` on a vault top. The kit's own file is never counted — a kit never
//! vendors itself (§15 D1012) — and what is counted is the files on disk, so an
//! open document's unsaved vault is not.

use std::path::PathBuf;
use std::sync::mpsc;

/// A count in flight or arrived, for one kit.
pub(crate) struct KitUsers {
    /// The kit's `meta.id`.
    pub kit: String,
    rx: mpsc::Receiver<usize>,
    /// The number, once the worker has it.
    pub count: Option<usize>,
    /// The worker ended without one (a panic on a library file): the dialog
    /// stops waiting and says what it can without a number.
    lost: bool,
    /// Set when the count is dropped, so the worker stops between files rather
    /// than loading the rest of the library for nobody — otherwise each reopening
    /// of the dialog starts another whole-library load beside it (§15 D1024).
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for KitUsers {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl KitUsers {
    /// Start counting the documents among `paths` that use `kit`.
    pub(crate) fn start(kit: String, paths: Vec<PathBuf>) -> Self {
        let (tx, rx) = mpsc::channel();
        let id = kit.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        std::thread::Builder::new()
            .name("ondin-kit-users".into())
            .spawn(move || {
                let mut n = 0;
                for p in &paths {
                    if stopped.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    n += usize::from(uses(p, &id));
                }
                // The dialog closed and dropped the receiver: nobody is asking.
                let _ = tx.send(n);
            })
            .expect("spawn the kit user count");
        KitUsers {
            kit,
            rx,
            count: None,
            lost: false,
            stop,
        }
    }

    /// Take the number if the worker has finished; answers whether it is still
    /// counting.
    pub(crate) fn pass(&mut self) -> bool {
        if self.counting() {
            match self.rx.try_recv() {
                Ok(n) => self.count = Some(n),
                Err(mpsc::TryRecvError::Disconnected) => self.lost = true,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.counting()
    }

    /// Whether the number is still on its way.
    pub(crate) fn counting(&self) -> bool {
        self.count.is_none() && !self.lost
    }

    /// Wait for the number — the tests', `Covers::settle`'s shape (§15 D820).
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        if self.count.is_none() {
            self.count = self
                .rx
                .recv_timeout(std::time::Duration::from_secs(20))
                .ok();
        }
    }
}

/// Whether the document at `path` holds a vault main from `kit`. A file that
/// cannot be read or loaded uses nothing.
fn uses(path: &std::path::Path, kit: &str) -> bool {
    let Some(doc) = std::fs::read(path)
        .ok()
        .and_then(|b| ondin_core::io::load(&b).ok())
    else {
        return false;
    };
    let Some(vault) = doc.vault().and_then(|v| doc.get(v)) else {
        return false;
    };
    vault.children().iter().any(|t| {
        doc.get(*t)
            .and_then(|n| n.origin())
            .is_some_and(|o| o.kit == kit)
    })
}

/// What the dialog says while the count is on its way: a sentence that reads
/// as an answer would be replaced by the true one a moment later.
pub(crate) const CHECKING: &str = "Checking kit usages…";

/// The dialog's first sentence: how many documents use the kit and what
/// happens to them, [`CHECKING`] while the count is still arriving, and what
/// happens without the number when none is coming.
pub(crate) fn sentence(name: &str, count: Option<usize>, counting: bool) -> String {
    match count {
        None if counting => CHECKING.to_string(),
        None => format!(
            "Documents that use components from {name} keep their copies, which keep \
             working, and stop receiving updates."
        ),
        Some(0) => format!("No document in your library uses components from {name}."),
        Some(1) => format!(
            "1 document in your library uses components from {name}. It keeps its copies, \
             which keep working, and stops receiving updates."
        ),
        Some(n) => format!(
            "{n} documents in your library use components from {name}. They keep their \
             copies, which keep working, and stop receiving updates."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kits::index::kits_in;
    use crate::kits::picker::tests::{app_on, file_kit, temp_root};
    use eframe::egui;
    use ondin_core::Document;

    /// **1D counts the documents that use the kit** (§15 D1021): a library of
    /// two kits, a document that placed one of the first's components and saved,
    /// and one that uses only the second. Opening *Stop sharing* on the first from
    /// the dashboard's card starts the count; it arrives as **1**, and closing the
    /// dialog drops it.
    ///
    /// Flip, run: `uses` answering for any vault top, never reading `origin.kit`,
    /// fails *"only the kit's users"*, 2 against 1. ⚠️ The second kit is what
    /// makes it bite: with *Notes* holding an empty vault instead, the same flip
    /// passed, an empty vault having no top to answer for.
    #[test]
    fn stop_sharing_counts_the_documents_that_use_the_kit() {
        let ctx = egui::Context::default();
        let dir = temp_root("users");
        let (kit, _, badge) = file_kit(&dir);
        let (second, _, second_badge) = file_kit(&dir);
        assert_ne!(kit, second, "two kits");
        let mut app = app_on(&ctx, dir.clone());
        // A second document, saved using the *other* kit only.
        let mut other = Document::new(app.session.ids.mint());
        let other_path =
            crate::library::store::file_document(&dir, None, "Notes", &mut other).unwrap();
        let second_published = app
            .library
            .entries
            .iter()
            .find(|e| e.meta.id.as_deref() == Some(second.as_str()))
            .and_then(|e| ondin_core::io::load(&std::fs::read(&e.path).unwrap()).ok())
            .and_then(|d| d.published().cloned())
            .unwrap();
        let mut ids = ondin_core::IdSource::new(0xBF);
        let (tx, _) = ondin_core::kit::insert(
            &other,
            &mut ids,
            &second,
            4,
            &second_published,
            second_badge,
            other.first_page(),
        )
        .unwrap();
        other.apply(&tx).unwrap();
        crate::library::store::write_document(&other_path, &other).unwrap();

        let k = kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit)
            .unwrap();
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let p = app.kit_shelf.current(&k).unwrap();
        let root = app.session.doc.first_page();
        let (tx, _) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &kit,
            k.version,
            &p,
            badge,
            root,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        assert!(app.save_file(false), "the user saved");
        app.library.refresh();

        app.unsharing_kit = Some(Some(k.path.clone()));
        let _ = ctx.run_ui(Default::default(), |_| app.stop_sharing_dialog(&ctx));
        let users = app.kit_users.as_mut().expect("the count started");
        assert_eq!(users.kit, kit);
        users.settle();
        assert_eq!(users.count, Some(1), "only the kit's users");

        app.unsharing_kit = None;
        let _ = ctx.run_ui(Default::default(), |_| app.stop_sharing_dialog(&ctx));
        assert!(app.kit_users.is_none(), "dropped with the dialog");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sentence reads the count, and says it is checking until it has one.
    #[test]
    fn the_sentence_says_the_count() {
        assert_eq!(sentence("Atlas", None, true), CHECKING);
        assert_eq!(
            sentence("Atlas", Some(0), false),
            "No document in your library uses components from Atlas."
        );
        assert!(sentence("Atlas", Some(1), false).starts_with("1 document in your library uses"));
        assert!(sentence("Atlas", Some(3), false).starts_with("3 documents in your library use "));
    }

    /// A worker that ended without a count stops the wait, and the sentence
    /// stands without the number rather than checking for good.
    ///
    /// flip: `pass` reading `Disconnected` as still counting → fails at "the
    /// wait ends".
    #[test]
    fn a_lost_count_stops_checking() {
        let (tx, rx) = mpsc::channel();
        drop(tx);
        let mut users = KitUsers {
            kit: "atlas".into(),
            rx,
            count: None,
            lost: false,
            stop: Default::default(),
        };
        assert!(!users.pass(), "the wait ends");
        let said = sentence("Atlas", users.count, users.counting());
        assert!(
            said.starts_with("Documents that use components from Atlas"),
            "{said}"
        );
    }
}
