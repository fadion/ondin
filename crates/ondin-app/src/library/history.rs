//! Undo history kept between sessions: `.history/<document id>.json`, beside the
//! library rather than in the document, so it never travels with a copy of the file.
//!
//! A history is only true of the exact document it was recorded on, and a file can
//! change while Ondin is not looking — a sync from another machine, a git merge, a
//! version restored. So the record carries a SHA-256 of the document's bytes as
//! they were written, and it is read back only when the file opened has the same
//! digest, and only by the build that wrote it (`ondin_core::Transaction`'s saved
//! form follows the model). Any other history is dropped without a word: losing
//! one costs undo steps, replaying one onto the wrong document would corrupt it.

use super::ids;
use ondin_core::SavedHistory;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// The histories' folder under the library root.
pub const HISTORY_DIR: &str = ".history";

/// The most a history file may hold, in bytes; the oldest undo steps go first,
/// then the farthest redo steps. An undone image delete carries the picture's
/// bytes, so a count of steps alone does not bound the file.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// What is written: the history and what it is true of.
#[derive(Serialize, Deserialize)]
struct Record {
    /// The app version that wrote it.
    build: String,
    /// The SHA-256 of the document's bytes as written, in hex.
    file: String,
    history: SavedHistory,
}

/// The SHA-256 of a document's bytes, in hex.
pub fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Where the history of the document with this id lives, or `None` for an id
/// that is not one this library minted (it is joined as a file name).
fn path(root: &Path, id: &str) -> Option<PathBuf> {
    ids::is_minted(id).then(|| root.join(HISTORY_DIR).join(format!("{id}.json")))
}

/// Write `history` as the history of the document `id`, whose bytes on disk have
/// `file` as their [`digest`]. An empty history removes the record.
pub fn write(root: &Path, id: &str, file: &str, history: SavedHistory) -> io::Result<()> {
    let Some(at) = path(root, id) else {
        return Ok(());
    };
    let history = within(history, MAX_BYTES);
    if history.undo.is_empty() && history.redo.is_empty() {
        remove(root, id);
        return Ok(());
    }
    let record = Record {
        build: env!("CARGO_PKG_VERSION").to_string(),
        file: file.to_string(),
        history,
    };
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    crate::atomic::write(&at, &bytes)
}

/// The history kept for the document `id` whose bytes have `file` as their
/// [`digest`], if one was written for exactly those bytes by this version.
pub fn read(root: &Path, id: &str, file: &str) -> Option<SavedHistory> {
    let bytes = std::fs::read(path(root, id)?).ok()?;
    let record: Record = serde_json::from_slice(&bytes).ok()?;
    (record.build == env!("CARGO_PKG_VERSION") && record.file == file).then_some(record.history)
}

/// Carry the history of the document `id` from the bytes digested `was` to those
/// digested `now`: the library rewrote the file's metadata (a rename, a move, a
/// kit mark) and the edits it records are as true of the one as of the other. A
/// history kept for other bytes is left alone.
pub fn restamp(root: &Path, id: &str, was: &str, now: &str) {
    let Some(at) = path(root, id) else {
        return;
    };
    let Some(mut record) = std::fs::read(&at)
        .ok()
        .and_then(|b| serde_json::from_slice::<Record>(&b).ok())
    else {
        return;
    };
    if record.file != was {
        return;
    }
    record.file = now.to_string();
    if let Ok(bytes) = serde_json::to_vec(&record) {
        let _ = crate::atomic::write(&at, &bytes);
    }
}

/// The history to resume for a document just read from `bytes`, if one is kept.
pub fn for_opened(root: &Path, doc: &ondin_core::Document, bytes: &[u8]) -> Option<SavedHistory> {
    read(root, doc.meta().id.as_deref()?, &digest(bytes))
}

/// Forget the history of a trashed document `id` deleted for good — unless a
/// live document carries the same id (a sync client's conflict copy, a file
/// copied by hand), since the record is keyed by id and is that one's too.
pub fn forget_purged(root: &Path, id: &str) {
    let live = super::scan::scan(root)
        .iter()
        .any(|e| e.meta.id.as_deref() == Some(id));
    if !live {
        remove(root, id);
    }
}

/// Forget the history of the document `id`.
pub fn remove(root: &Path, id: &str) {
    if let Some(at) = path(root, id) {
        let _ = std::fs::remove_file(at);
    }
}

/// `history` cut to at most `max` bytes as serialised: a step that alone is over
/// `max` goes first, with every step beyond it that needs it (older undo steps,
/// farther redo steps); then the oldest undo steps, then the farthest redo steps.
/// Without the first cut, one undone large picture would drain every small step.
fn within(mut history: SavedHistory, max: usize) -> SavedHistory {
    let size =
        |step: &ondin_core::SavedStep| serde_json::to_vec(step).map_or(usize::MAX, |b| b.len());
    let mut undo: Vec<usize> = history.undo.iter().map(size).collect();
    let mut redo: Vec<usize> = history.redo.iter().map(size).collect();
    if let Some(at) = undo.iter().rposition(|s| *s > max) {
        history.undo.drain(..=at);
        undo.drain(..=at);
    }
    if let Some(at) = redo.iter().position(|s| *s > max) {
        history.redo.truncate(at);
        redo.truncate(at);
    }
    let mut total: usize = undo
        .iter()
        .chain(&redo)
        .fold(0, |a, b| a.saturating_add(*b));
    let mut cut = 0;
    while total > max && cut < undo.len() {
        total -= undo[cut];
        cut += 1;
    }
    history.undo.drain(..cut);
    let mut keep = redo.len();
    while total > max && keep > 0 {
        keep -= 1;
        total -= redo[keep];
    }
    history.redo.truncate(keep);
    history
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{SavedStep, StepNote, Transaction};

    fn step(label: &str) -> SavedStep {
        SavedStep {
            tx: Transaction::default(),
            note: StepNote {
                label: label.into(),
                ..StepNote::default()
            },
        }
    }

    fn labels(steps: &[SavedStep]) -> Vec<String> {
        steps.iter().map(|s| s.note.label.clone()).collect()
    }

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A record is read back for the bytes it was written against and for nothing
    /// else; an empty history removes it.
    /// flip: `read` without its digest test → fails at "other bytes".
    #[test]
    fn a_history_is_read_back_only_for_the_bytes_it_was_written_against() {
        let dir = temp_root("read");
        let root = dir.as_path();
        let saved = SavedHistory {
            undo: vec![step("Fill")],
            redo: vec![step("Move")],
        };
        let file = digest(b"the document");
        write(root, ID, &file, saved.clone()).unwrap();
        assert_eq!(read(root, ID, &file), Some(saved));
        assert_eq!(read(root, ID, &digest(b"other bytes")), None, "other bytes");

        // Written by another version, the same bytes' record is not read.
        let at = root.join(HISTORY_DIR).join(format!("{ID}.json"));
        let mut record: Record = serde_json::from_slice(&std::fs::read(&at).unwrap()).unwrap();
        record.build = "0.0.0-another".into();
        std::fs::write(&at, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(read(root, ID, &file), None, "another version");

        write(root, ID, &file, SavedHistory::default()).unwrap();
        assert!(!root.join(HISTORY_DIR).join(format!("{ID}.json")).exists());
    }

    /// Purging a trashed document keeps the record while a live document carries
    /// the same id, and forgets it once none does.
    /// flip: `forget_purged` without its live check → fails at "kept".
    #[test]
    fn a_purge_keeps_the_record_a_live_copy_shares() {
        let dir = temp_root("purge");
        let root = dir.as_path();
        let mut doc = ondin_core::Document::new(ondin_core::IdSource::new(3).mint());
        let mut meta = doc.meta().clone();
        meta.id = Some(ID.into());
        doc.set_meta(meta);
        let live = root.join("Copy.ondin");
        crate::library::store::write_document(&live, &doc).unwrap();
        let saved = SavedHistory {
            undo: vec![step("Fill")],
            redo: Vec::new(),
        };
        write(root, ID, &digest(b"x"), saved).unwrap();
        let record = root.join(HISTORY_DIR).join(format!("{ID}.json"));
        forget_purged(root, ID);
        assert!(record.exists(), "kept");
        std::fs::remove_file(&live).unwrap();
        forget_purged(root, ID);
        assert!(!record.exists(), "forgotten");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An id that is not one the library mints is never joined into a path.
    #[test]
    fn a_foreign_id_writes_nothing() {
        let dir = temp_root("foreign");
        let saved = SavedHistory {
            undo: vec![step("Fill")],
            redo: Vec::new(),
        };
        write(&dir, "../escape", "x", saved).unwrap();
        assert!(!dir.join(HISTORY_DIR).exists());
    }

    /// Over the byte ceiling the oldest undo steps go first, then the farthest redo.
    #[test]
    fn an_oversized_history_drops_its_oldest_steps_first() {
        let history = SavedHistory {
            undo: vec![step("A"), step("B"), step("C")],
            redo: vec![step("D"), step("E")],
        };
        let one = serde_json::to_vec(&step("A")).unwrap().len();
        let cut = within(history.clone(), one * 4);
        assert_eq!(labels(&cut.undo), ["B", "C"]);
        assert_eq!(labels(&cut.redo), ["D", "E"]);
        let cut = within(history, one);
        assert!(cut.undo.is_empty());
        assert_eq!(labels(&cut.redo), ["D"]);
    }

    /// A step that alone is over the ceiling goes with the steps that need it,
    /// and takes none of the small ones that fit.
    ///
    /// flip: `within` without its oversized-step cut → fails at the undo steps.
    #[test]
    fn one_oversized_step_drains_nothing_else() {
        let big = "x".repeat(1000);
        let history = SavedHistory {
            undo: vec![step("A"), step("B"), step("C")],
            redo: vec![step("D"), step(&big), step("F")],
        };
        let one = serde_json::to_vec(&step("A")).unwrap().len();
        let cut = within(history, one * 5);
        assert_eq!(labels(&cut.undo), ["A", "B", "C"]);
        assert_eq!(labels(&cut.redo), ["D"], "the redo past it needs it");

        let history = SavedHistory {
            undo: vec![step("A"), step(&big), step("C")],
            redo: vec![step("D")],
        };
        let cut = within(history, one * 5);
        assert_eq!(labels(&cut.undo), ["C"], "the undo before it needs it");
        assert_eq!(labels(&cut.redo), ["D"]);
    }
}
