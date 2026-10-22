//! The one background thread every record this app keeps is written on
//! (§15 D392, D393).
//!
//! [`crate::atomic`] is how a record is written; this is where. The two writes
//! made on a timer over a whole document (autosave and the crash snapshot) must
//! not run from the frame: `ondin_core::io::save` serialises every embedded
//! picture, which costs over a second in a debug build at 60 MB.
//!
//! Moving them is cheap because [`Document`] is cheap to clone: an
//! `ImageSource::Embedded` holds an `Arc<[u8]>` (§15 D301), so the copy handed to
//! the worker is a refcount bump per picture.
//!
//! One worker serves both, in FIFO order, and the only other writer is the frame
//! itself, which [`Writer::settle`]s first (see `OndinApp::save_file`). The FIFO
//! also keeps a removal from being overtaken by the write it may be cancelling,
//! so *Discard* and `OndinApp::drop_recovery` cannot resurrect a deleted
//! snapshot. The write layer does not depend on the writer count: `atomic::temp_for`
//! is unique per write, since a second Ondin process can race too (§15 D548).
//!
//! The queue is drained, not polled for a picture: a [`Done`] only moves the save
//! dot and its bookkeeping. It still calls `request_repaint`, for
//! `crate::fonts::FontService::new`'s reason, since eframe is reactive.

use super::{recovery, store};
use ondin_core::Document;
use std::path::{Path, PathBuf};

/// The queue onto the writer thread, and the answers coming back.
pub struct Writer {
    /// `None` only while [`Drop`] is taking it — dropping the sender is what ends
    /// the worker's `for` loop.
    tx: Option<std::sync::mpsc::Sender<Job>>,
    done: std::sync::mpsc::Receiver<Done>,
    /// Jobs sent and not yet answered.
    ///
    /// Counted here because a `Receiver` cannot say how many sends are still in
    /// the worker's hands, which is what [`Self::settle`] needs.
    in_flight: usize,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Answers this side made up, waiting to be drained with the worker's.
    ///
    /// A dead worker sends nothing, and a [`Done`] is the only route to the status
    /// line (crash recovery's one-shot latch, `recovery::RecoveryState::warned`,
    /// D377, and a save's failure message). Both places that notice a death put a
    /// failure here instead of staying silent.
    reports: Vec<Done>,
}

/// One job on [`Writer`]'s queue.
///
/// The documents are boxed so a queued `Forget` (two strings) does not carry a
/// `Document`'s worth of stack.
enum Job {
    /// Serialise `doc` and write it as the crash snapshot for `key`
    /// ([`recovery::write`]).
    Snapshot {
        root: PathBuf,
        key: String,
        doc: Box<Document>,
        /// The revision `doc` was cloned at, handed back on success.
        at: u64,
    },
    /// [`recovery::remove`], on the queue so that it cannot overtake a snapshot.
    Forget { root: PathBuf, key: String },
    /// Serialise `doc` over its existing path ([`store::write_document`]).
    Document {
        path: PathBuf,
        doc: Box<Document>,
        /// As `Snapshot`'s; it also decides whether the session may be marked
        /// clean, since an edit made in flight leaves the file behind.
        at: u64,
    },
}

/// The one document path the worker will panic on instead of writing.
///
/// The fault is injected because no panic in the save path is known. What a test
/// reaches is its residue: the thread gone, that job's `Done` never sent, and
/// `in_flight` still counting it, as a panic in `ondin_core::io::save`,
/// `store::write_document` or `recovery::write` would leave it.
///
/// Keyed on a path, not a one-shot "next job" flag: the test binary is one
/// process and a one-shot would fire in whichever test autosaved next. The panic
/// message reaching stderr is cheaper than swapping the global panic hook.
#[cfg(test)]
static POISONED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Make the worker panic when it dequeues a `Job::Document` for `path`, rather
/// than writing it. See `POISONED`.
#[cfg(test)]
pub fn poison_document(path: &Path) {
    if let Ok(mut poisoned) = POISONED.lock() {
        *poisoned = Some(path.to_path_buf());
    }
}

/// What a finished job reports back.
///
/// Five variants rather than one `Result` per job: a snapshot failure latches
/// and a save failure does not, and collapsing them would hide a save's error
/// behind crash recovery's one-shot warning.
pub enum Done {
    /// A snapshot capturing this revision is on disk.
    Snapshotted(u64),
    /// It is not, and this is why — for the latch, once.
    SnapshotFailed(String),
    /// A removal finished. Nothing reads it; [`Writer::settle`] counts it.
    Forgot,
    /// The document at `path` now holds revision `at`.
    Saved { path: PathBuf, at: u64 },
    /// It does not, and this is why.
    SaveFailed(String),
}

impl Writer {
    /// Start the worker.
    ///
    /// Called on the first job, so the app pays for a thread only once a
    /// document has been edited.
    pub fn spawn(ctx: &egui::Context) -> Self {
        let (tx, jobs) = std::sync::mpsc::channel::<Job>();
        let (report, done) = std::sync::mpsc::channel::<Done>();
        let ctx = ctx.clone();
        let handle = std::thread::spawn(move || {
            for job in jobs {
                let msg = match job {
                    Job::Snapshot { root, key, doc, at } => {
                        match recovery::write(&root, &key, &doc) {
                            Ok(_) => Done::Snapshotted(at),
                            Err(e) => Done::SnapshotFailed(e.to_string()),
                        }
                    }
                    Job::Forget { root, key } => {
                        recovery::remove(&root, &key);
                        Done::Forgot
                    }
                    Job::Document { path, doc, at } => {
                        #[cfg(test)]
                        if POISONED
                            .lock()
                            .is_ok_and(|p| p.as_deref() == Some(path.as_path()))
                        {
                            panic!("synthetic writer panic — writer::poison_document({path:?})");
                        }
                        match store::write_document(&path, &doc) {
                            Ok(()) => Done::Saved { path, at },
                            Err(e) => Done::SaveFailed(e.to_string()),
                        }
                    }
                };
                if report.send(msg).is_err() {
                    return; // the app is gone
                }
                ctx.request_repaint();
            }
        });
        Self {
            tx: Some(tx),
            done,
            in_flight: 0,
            handle: Some(handle),
            reports: Vec::new(),
        }
    }

    /// Queue a crash snapshot of `doc`, taken at revision `at`.
    pub fn snapshot(&mut self, root: &Path, key: &str, doc: Document, at: u64) {
        self.send(Job::Snapshot {
            root: root.to_path_buf(),
            key: key.to_owned(),
            doc: Box::new(doc),
            at,
        });
    }

    /// Queue the removal of the snapshot filed under `key`.
    pub fn forget(&mut self, root: &Path, key: &str) {
        self.send(Job::Forget {
            root: root.to_path_buf(),
            key: key.to_owned(),
        });
    }

    /// Queue a write of `doc` over `path`, taken at revision `at`.
    pub fn document(&mut self, path: &Path, doc: Document, at: u64) {
        self.send(Job::Document {
            path: path.to_path_buf(),
            doc: Box::new(doc),
            at,
        });
    }

    fn send(&mut self, job: Job) {
        // A send fails only if the worker died by panicking inside a job. It is
        // reported against the job that could not be sent, not dropped: every
        // write of that kind this session is lost, and `settle` finding an empty
        // queue says nothing, so a `Done` is the only thing a status line reads.
        let failure = match &job {
            Job::Document { .. } => Done::SaveFailed(WORKER_STOPPED.to_string()),
            _ => Done::SnapshotFailed(WORKER_STOPPED.to_string()),
        };
        match self.tx.as_ref().map(|tx| tx.send(job)) {
            Some(Ok(())) => self.in_flight += 1,
            _ => self.reports.push(failure),
        }
    }

    /// Everything the worker has finished, without waiting for what it has not.
    ///
    /// This reads `try_recv`'s error variant rather than collapsing it (§15 D549):
    /// treating `Disconnected` as idle would let a worker that panicked in a
    /// `Job::Document` disable autosave silently, since `EditorSession::saving`
    /// stays `Some` and `OndinApp::autosave_tick`'s `is_saving()` gate returns
    /// early on every frame, so [`Self::send`]'s failure report is never reached.
    pub fn drain(&mut self) -> Vec<Done> {
        // This side's own answers first, see [`Self::reports`].
        let mut out: Vec<Done> = std::mem::take(&mut self.reports);
        loop {
            match self.done.try_recv() {
                Ok(msg) => {
                    // Saturating: an overflow would take a debug build down on
                    // every frame, and the count should not go under anyway.
                    self.in_flight = self.in_flight.saturating_sub(1);
                    out.push(msg);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // The worker is gone and whatever was counted died with it;
                // [`Self::settle`]'s `Err(_)` arm on the frame path, reporting both
                // failures as it cannot tell which job it was.
                //
                // Zeroing `in_flight` is the latch that stops every later frame
                // reporting the same death. `Disconnected` with nothing in flight
                // is a death between jobs, which `send` reports on its next attempt.
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if self.in_flight > 0 {
                        self.in_flight = 0;
                        out.push(Done::SnapshotFailed(WORKER_DIED.to_string()));
                        out.push(Done::SaveFailed(WORKER_DIED.to_string()));
                    }
                    break;
                }
            }
        }
        out
    }

    /// Everything the worker has finished and everything still queued, blocking
    /// until the queue is empty.
    ///
    /// Never for an ordinary frame, which must not wait on a serialise. Only a
    /// deliberate close (no next frame to notice), `OndinApp::save_file` (an
    /// autosave at an older revision could otherwise land after the explicit save
    /// and put the earlier document on disk; §15 D548) and a test asserting on
    /// the file.
    pub fn settle(&mut self) -> Vec<Done> {
        let mut out = self.drain();
        while self.in_flight > 0 {
            match self.done.recv() {
                Ok(msg) => {
                    self.in_flight -= 1;
                    out.push(msg);
                }
                // The worker panicked inside a job, so nothing more is coming and
                // waiting would hang the close. The count is cleared and the loss
                // reported, for [`Self::reports`]' reason; `send` would only notice
                // on a next attempt that a close may never make. Reported as both
                // kinds, as the count is all that is left of the job: saying it
                // twice beats a silent lost save or silently disabled recovery.
                Err(_) => {
                    self.in_flight = 0;
                    out.push(Done::SnapshotFailed(WORKER_DIED.to_string()));
                    out.push(Done::SaveFailed(WORKER_DIED.to_string()));
                    break;
                }
            }
        }
        out
    }

    /// Wait for the worker to stop, for a test that has queued a job
    /// `poison_document` will kill it on.
    ///
    /// Joined rather than slept on, so the fixture is not a race: afterwards
    /// `try_recv` answers `Disconnected` and `in_flight` still counts the dead
    /// job. Taking the handle leaves `Drop` nothing to join.
    #[cfg(test)]
    pub fn wait_for_worker(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// What a failed `send` says; one string for both `Done`s it can become.
const WORKER_STOPPED: &str = "the writer thread stopped and cannot be restarted this session";

/// What a hang-up mid-job says.
const WORKER_DIED: &str = "the writer thread stopped part-way through a write";

impl Drop for Writer {
    /// Joins, so dropping the app flushes a write still in progress; the wait is
    /// bounded by one document's serialise.
    ///
    /// This is the backstop, not the primary (`eframe::App::on_exit`): eframe
    /// calls `std::process::exit(0)` when `run_and_return` is off, and on macOS
    /// `Cmd-Q` `run_app_on_demand` never returns, so the app is never dropped.
    /// `on_exit` is reached on all paths, then the close arms, then this.
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
