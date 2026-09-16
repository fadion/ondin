//! The one background thread every record this app keeps is written on
//! (§15 D392, D393).
//!
//! **[`crate::atomic`] is *how* a record is written; this is *where*.** That
//! module's rule is that a file worth an fsync is one nothing else can
//! reconstruct — the document, its pinned versions, the project list, the
//! preferences, the per-machine index, a crash snapshot. This one adds the
//! second half of the same argument: the two of those that are written *on a
//! timer, over a whole document* must not be written from the frame.
//!
//! **Why a thread at all.** `ondin_core::io::save` serialises the whole
//! document, every embedded picture base64'd into the JSON, and until 2026-08-30
//! both the crash snapshot and the autosave ran straight from the frame that
//! noticed their interval had elapsed. Measured at four sizes of embedded image
//! rather than estimated: 1 MB of pictures costs 1.3 ms in release and 20 ms in
//! debug, 20 MB costs 25 ms and 387 ms, and 60 MB costs 72 ms and **1.15 s**,
//! before the disk write. So a photo-carrying document stalled the UI for a
//! second or more every ten seconds for the snapshot and every thirty for the
//! save, in the build anyone develops in — the two features that exist to
//! protect work being the most expensive thing in the frame.
//!
//! **What makes moving them cheap is [`Document`] being cheap to clone.** An
//! `ImageSource::Embedded` holds an `Arc<[u8]>` (§15 D301), so the copy handed
//! to the worker is a refcount bump per picture and not the pictures. The frame
//! keeps the clone and gives away the serialise, which is the right way round by
//! three orders of magnitude.
//!
//! **One worker for both.** With one queue there is exactly one background
//! writer of anything, in FIFO order, and the only other writer is the frame
//! itself — which [`Writer::settle`]s first (see `OndinApp::save_file`).
//!
//! ⚠️ **That used to be load-bearing for [`crate::atomic`]'s temp-file scheme and
//! is not any more (§15 D548).** `atomic::temp_for` derived the temp name from the
//! target with no unique part, on the stated grounds that this app does not have
//! two writers racing for one path — so a second thread here would have made that
//! false the day it was added, and this paragraph was the thing holding it shut by
//! hand. It was **already** false, for a reason no amount of care in this module
//! could reach: nothing stops a second Ondin *process*, and `[A3-L5-04]` staged
//! the torn document that follows. `temp_for` now carries a pid and a counter, so
//! the write layer no longer depends on how many writers this one has. **The FIFO
//! below still does**, which is why the queue is still the shape it is.
//!
//! ⚠️ **The FIFO is load-bearing a second time, for the snapshot.** A removal
//! goes down the same queue as the write it may be cancelling, so *Discard* and
//! `OndinApp::drop_recovery` cannot be overtaken by a write already in flight and
//! resurrect the file they just deleted — which at the next launch is a question
//! about work the user already dealt with.
//!
//! **The queue is drained, not polled for a picture.** Nothing a job reports
//! changes what is on screen by itself: a [`Done`] moves the save pill and the
//! bookkeeping behind it. It still calls `request_repaint`, for
//! `crate::fonts::FontService::new`'s reason — eframe here is reactive, and the
//! frame that would otherwise read the result is one that has to happen for some
//! other reason. The pill going from *Saving…* to *Saved · just now* is the one
//! visible thing that would otherwise wait for a mouse to move.

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
    /// **Counted here rather than derived from the channel**, because a
    /// `Receiver` cannot say how many sends are still in the worker's hands —
    /// which is the one question [`Self::settle`] exists to answer.
    in_flight: usize,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Answers this side made up, waiting to be drained with the worker's.
    ///
    /// ⚠️ **A dead worker sends nothing, and silence is the one thing neither of
    /// these features may do.** A [`Done`] is the only route to the status line —
    /// to crash recovery's one-shot latch (`recovery::RecoveryState::warned`,
    /// D377) and to a save's own failure message — so a worker that has panicked
    /// would otherwise take both down without a word. Both places that can notice
    /// put a failure here instead of shrugging.
    reports: Vec<Done>,
}

/// One job on [`Writer`]'s queue.
///
/// ⚠️ **The documents are boxed.** `Forget` is two strings and a `Document` is
/// not, so every queued removal would otherwise carry a document's worth of
/// stack — and removals are the common case at the end of a session.
enum Job {
    /// Serialise `doc` and write it as the crash snapshot for `key`
    /// ([`recovery::write`]).
    Snapshot {
        root: PathBuf,
        key: String,
        doc: Box<Document>,
        /// The revision `doc` was cloned at, handed back on success so the app
        /// records what reached the disk rather than what it hoped would.
        at: u64,
    },
    /// [`recovery::remove`], on the queue so that it cannot overtake a snapshot.
    Forget { root: PathBuf, key: String },
    /// Serialise `doc` over its existing path ([`store::write_document`]).
    Document {
        path: PathBuf,
        doc: Box<Document>,
        /// As `Snapshot`'s — and here it decides whether the session may be
        /// marked clean at all, since an edit made while this was in flight
        /// means the file is already behind.
        at: u64,
    },
}

/// The one document path the worker will panic on instead of writing.
///
/// ⚠️ **The fault has to be injected, because no panic in the save path is
/// known** — which is what kept `[S1.2-L1-04]` untested rather than unnoticed, and
/// D392 recorded that neither of its failure arms had a test. What a test reaches
/// through this is not "a panic" but its residue, and the residue is the whole
/// mechanism: the thread gone, that job's `Done` never sent, and `in_flight`
/// still counting it. A panic in `ondin_core::io::save`, in
/// `store::write_document` or in `recovery::write` arrives here identically.
///
/// ⚠️ **Keyed on a path rather than being a bare "next job" one-shot**, which is
/// where this differs from `boolean::poison_next` and why. That one can be a
/// global flag because nothing else evaluates a boolean concurrently; here the
/// whole test binary is one process, a document write is the commonest job in it,
/// and a one-shot would be consumed by whichever test happened to autosave next —
/// arming the fault in one test and firing it in another. A path inside the test's
/// own temp directory cannot be.
///
/// The panic message reaches the test's stderr, which is the ordinary cost of a
/// synthetic panic here and cheaper than swapping the process-global panic hook
/// out from under whatever tests are running beside it.
#[cfg(test)]
static POISONED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Make the worker panic when it dequeues a `Job::Document` for `path`, rather
/// than writing it. See `POISONED`.
///
/// Plain backticks throughout this item and the two below it, not `[links]` —
/// §15 D319's convention: they are `#[cfg(test)]`, so `cargo doc` cannot see them
/// and a link here is decoration no gate can validate.
#[cfg(test)]
pub fn poison_document(path: &Path) {
    if let Ok(mut poisoned) = POISONED.lock() {
        *poisoned = Some(path.to_path_buf());
    }
}

/// What a finished job reports back.
///
/// **Five variants and five readers, rather than one `Result` per job.** A
/// snapshot failing and a save failing are different sentences in different
/// places — one latches, one does not — and collapsing them would put a save's
/// error behind crash recovery's one-shot warning, where the user would see it
/// once and never again.
pub enum Done {
    /// A snapshot capturing this revision is on disk.
    Snapshotted(u64),
    /// It is not, and this is why — for the latch, once.
    SnapshotFailed(String),
    /// A removal finished. **Nothing reads this**; it exists so that
    /// [`Writer::settle`] can count every job the queue has taken.
    Forgot,
    /// The document at `path` now holds revision `at`.
    Saved { path: PathBuf, at: u64 },
    /// It does not, and this is why.
    SaveFailed(String),
}

impl Writer {
    /// Start the worker.
    ///
    /// Called on the first job and not before, so the app pays for a thread only
    /// once a document has been edited — which is most sessions, and none of the
    /// several hundred headless probes that never dirty anything.
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
        // A send fails only if the worker has died, which it does by panicking
        // inside a job — nothing else ends it while this sender is alive.
        //
        // ⚠️ **Reported rather than dropped, and reported against the job that
        // could not be sent**, because the two failures are not the same
        // sentence. Either way it is not one write lost but every write of that
        // kind this session will ever make: the worker cannot be restarted.
        //
        // An earlier draft shrugged, on the argument that the next `settle`
        // would say so by finding the queue empty. That was wrong — `settle`
        // finds it empty and says nothing to anybody, and a `Done` is the only
        // thing either status line reads. `arch-scribe` caught it by reading the
        // comment against what `settle` actually does.
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
    /// ⚠️ **This reads `try_recv`'s error variant rather than collapsing it, and
    /// until §15 D549 it did not (`[S1.2-L1-04]`).** `while let Ok(msg) =
    /// try_recv()` leaves on `Empty` and on `Disconnected` alike, so the
    /// once-per-frame path — the only one an ordinary editor session ever runs —
    /// read a hung-up channel as an idle one. A worker that panicked inside a
    /// `Job::Document` then took autosave down for the rest of the session in
    /// silence: that job's `Done` never arrives, `EditorSession::saving` stays
    /// `Some`, and `OndinApp::autosave_tick`'s `is_saving()` gate returns early on
    /// every subsequent frame — so no further document job is ever queued and
    /// [`Self::send`]'s failure report, which D392 names as the backstop, is never
    /// reached. The pill reads *Saving…* forever while nothing is being saved.
    /// [`Self::settle`] has always noticed; `settle` runs only from `save_file`,
    /// `on_exit` and `relocate`, none of which happen on their own.
    pub fn drain(&mut self) -> Vec<Done> {
        // This side's own answers first — see [`Self::reports`].
        let mut out: Vec<Done> = self.reports.drain(..).collect();
        loop {
            match self.done.try_recv() {
                Ok(msg) => {
                    // Saturating, though the count cannot go under: the worker
                    // sends exactly one `Done` per job and nothing else holds the
                    // sender. A wrong answer here is one line of bookkeeping; a
                    // subtraction overflow is the app going down in a debug build,
                    // and this runs on every frame of every editor session.
                    self.in_flight = self.in_flight.saturating_sub(1);
                    out.push(msg);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                // The worker is gone and anything still counted died with it —
                // [`Self::settle`]'s `Err(_)` arm, on the frame path, for its
                // reasons: the count is all that is left of the lost job, so both
                // failures are reported because this cannot tell which it was.
                //
                // ⚠️ **`in_flight` is the latch and no new field is needed.**
                // Clearing it to zero is what stops the next frame — and every
                // frame after, since the channel stays disconnected forever —
                // reporting the same death again. A `Disconnected` with nothing in
                // flight is a worker that died *between* jobs, where no answer was
                // owed and `send` will report on the next attempt.
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

    /// Everything the worker has finished **and everything still queued**,
    /// blocking until the queue is empty.
    ///
    /// ⚠️ **For the way out of the process, for the frame that is about to write
    /// the same file itself, and for tests — never for an ordinary frame.** The
    /// whole point of this type is that a frame does not wait on a serialise.
    /// The three that must are a deliberate close, where there is no next frame
    /// to notice; `OndinApp::save_file`, which would otherwise be a second writer
    /// racing the worker for one path; and a test asserting on the file.
    ///
    /// ⚠️ **That middle reason used to add *"and one temp file"* and no longer
    /// can** (§15 D548): `atomic::temp_for` is unique per write, so the two do not
    /// collide there any more. **The reason is unchanged in force and narrower in
    /// kind** — an autosave taken at an older revision can still land *after* the
    /// explicit save and put the earlier document on disk. Losing the temp-file
    /// half of the argument is not a reason to relax this call.
    pub fn settle(&mut self) -> Vec<Done> {
        let mut out = self.drain();
        while self.in_flight > 0 {
            match self.done.recv() {
                Ok(msg) => {
                    self.in_flight -= 1;
                    out.push(msg);
                }
                // The worker panicked *inside* a job, so that job's answer will
                // never come. Nothing more is coming at all, and waiting would
                // hang the close this is usually being called from — so the
                // count is cleared and the loss is said out loud, for
                // [`Self::reports`]' reason. This is the one arm that notices a
                // death mid-job; `send` notices it on the next attempt, and
                // after a close there may not be one.
                //
                // ⚠️ **Reported as *both*, because this arm cannot tell which
                // job died.** The count is all that is left of it by here. Saying
                // it twice is the safe direction: the alternative is a save that
                // silently never happened, or crash recovery silently off.
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
    /// **Joined rather than slept on**, which is what makes such a fixture a
    /// measurement instead of a race: after this returns the worker's
    /// `Sender<Done>` has been dropped, so `try_recv` answers `Disconnected`
    /// rather than `Empty` on the very next call — and `in_flight` still counts
    /// the job that died, which together are the exact state
    /// `Self::drain` was blind to. Taking the handle also means `Drop` has
    /// nothing left to join, which is correct: the thread it would wait for has
    /// already ended.
    #[cfg(test)]
    pub fn wait_for_worker(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// What a failed `send` says. One string, because the two `Done`s it can become
/// differ in where the message lands and not in what happened.
const WORKER_STOPPED: &str = "the writer thread stopped and cannot be restarted this session";

/// What a hang-up mid-job says.
const WORKER_DIED: &str = "the writer thread stopped part-way through a write";

impl Drop for Writer {
    /// ⚠️ **Joins, which makes dropping the app a flush.** A write still in
    /// progress when the process leaves is the one loss both of these features
    /// exist to prevent. The wait is bounded by one document's serialise, and
    /// only happens when something was actually in flight.
    ///
    /// ⚠️ **The backstop, not the primary — that is `eframe::App::on_exit`, and
    /// getting the two the wrong way round is how this comment first read.** A
    /// destructor is the *most* skippable of the three flushes, not the least:
    /// eframe calls `std::process::exit(0)` when `run_and_return` is off, and
    /// even with it on, eframe's own note at `native/run.rs`'s `exiting` says
    /// that a macOS `Cmd-Q` reaches the loop's exit and then `run_app_on_demand`
    /// *never returns* — so the app is never dropped and this never runs. Two
    /// paths, not one. `on_exit` is reached on all of them, which is why the
    /// order is: `on_exit` first, the close arms for the case that has already
    /// decided, and this for whatever is left.
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
