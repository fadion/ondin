//! What a document knows about itself as a library entry: its name, project and
//! creation time (§5.11a).
//!
//! This is the only document state that is not about drawing. It lives in the
//! file because the library's base folder can be a synced directory; facts that
//! are local (last opened, thumbnails, the index) stay in the app's cache.
//!
//! Nothing here may change as a side effect of editing or saving, or byte-identical
//! re-saves (invariant 9) end for every document. A field written by a clock
//! belongs in the cache.
//!
//! Not an `Operation` and outside undo: renaming is a dashboard action, often on a
//! document that is not open. `Document::set_meta` writes it directly, and the
//! dirty flag is the app's business (`crate::document::Document::meta`).

use serde::{Deserialize, Serialize};

/// A document's library identity. Every field is optional, and all-`None` is the
/// state of every file written before this existed.
///
/// Optional rather than defaulted: a missing name means "fall back to the
/// filename", and collapsing that into an empty name would lose an old
/// document's stem on first save.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMeta {
    /// The library-wide identity, minted once and never rewritten.
    ///
    /// Version history and the thumbnail cache are keyed on it, so a rename does
    /// not orphan them; *Duplicate* mints a new one. Minted by the app, never by
    /// `io::load`, which must stay deterministic: a pre-library file loads with
    /// `None` and gets its id the first time the library writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The name the user typed — "My cool design", not `my-cool-design`. Stored
    /// because the filename stem is a lossy slug (`ondin-app`'s `library::naming`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The id of the project this document belongs to, if any.
    ///
    /// The grouping lives here, not in the folder layout: moving a file between
    /// folders does not move it between projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// When the document was created, in whole seconds since the Unix epoch.
    ///
    /// Stored rather than read off the filesystem, whose creation time is reset by
    /// copying into a synced folder and absent on most Linux filesystems. Seconds,
    /// not a formatted date, so no timezone enters. Written once at creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
    /// The kit mark (§5.3e), present when this document is a kit, published or not.
    ///
    /// It sits in the block the library scan reads from a file's first 4 KB
    /// (`io::probe`), so "is it a kit" and "which version" need no full load. The
    /// frozen published state is `Document::published`, at the end of the file.
    /// Written by *Make this a kit*, *Stop sharing* and *Publish*, never by a save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kit: Option<KitMark>,
}

/// A kit's mark and what it last published (§5.3e). Version 0 with no date is a
/// kit that has never published.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KitMark {
    /// The last publish's version, counting from 1; 0 before the first. Carried
    /// on when a kit stops sharing and is made one again, so a consumer's copies
    /// are never newer than the kit.
    #[serde(default)]
    pub version: u32,
    /// When the last publish was made, in whole seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<u64>,
    /// The last publish's note, when one was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// False once the kit has stopped sharing: the mark stays, but consumers read
    /// the document as no longer a kit.
    #[serde(default = "sharing_by_default", skip_serializing_if = "is_sharing")]
    pub sharing: bool,
}

fn sharing_by_default() -> bool {
    true
}

fn is_sharing(b: &bool) -> bool {
    *b
}

/// A fresh mark: sharing, never published — what *Make this a kit* writes, and
/// what serde's per-field defaults agree with.
impl Default for KitMark {
    fn default() -> Self {
        Self {
            version: 0,
            date: None,
            note: None,
            sharing: true,
        }
    }
}

impl KitMark {
    /// Whether consumers should treat the document as a kit right now.
    pub fn is_shared(&self) -> bool {
        self.sharing
    }
}

impl DocumentMeta {
    /// Whether nothing has been recorded — the state of every file written
    /// before this block existed.
    ///
    /// `DocumentDto` skips the whole object when this is true, so a document that
    /// never went through the library serializes to the bytes it always did
    /// (invariant 9).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `s` has the shape [`Self::id`] is minted in (§15 D419): exactly
    /// [`ID_HEX_LEN`] lowercase hex characters, as `ondin-app`'s
    /// `library::ids::mint` writes them.
    ///
    /// A security check: the id is joined as a path component by the crash
    /// snapshot, version pin and cover cache writers, so `"id": "../x"` would
    /// make them write over a real document, and on Windows an absolute id makes
    /// `Path::join` discard the base. The cover cache reaches every entry the
    /// dashboard draws, so merely opening the library is enough to trigger it.
    pub fn is_wellformed_id(s: &str) -> bool {
        s.len() == ID_HEX_LEN
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// Drop anything in the block that a *file* is not allowed to assert.
    ///
    /// Today that is [`Self::id`] alone. It is dropped rather than refusing the
    /// document, since `None` is an ordinary state and the library mints a fresh
    /// id on the next write.
    ///
    /// `name` and `project` are left alone: a name never becomes a path
    /// (`library::naming::slug` stands between), and a project id is looked up in
    /// `projects.json`, not joined.
    pub(crate) fn sanitized(mut self) -> Self {
        if self
            .id
            .as_deref()
            .is_some_and(|s| !Self::is_wellformed_id(s))
        {
            self.id = None;
        }
        self
    }
}

/// The length of a well-formed [`DocumentMeta::id`] in hex characters.
///
/// Mirrors `ondin-app`'s `library::ids::ID_HEX_LEN`: minting lives in the app,
/// validation must be in core, where loading happens.
pub const ID_HEX_LEN: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_meta_is_empty_and_every_field_leaves_it() {
        assert!(DocumentMeta::default().is_empty());
        // Each field alone must leave the empty state (`is_empty` compares with `==`).
        for m in [
            DocumentMeta {
                id: Some("a".into()),
                ..Default::default()
            },
            DocumentMeta {
                name: Some("My cool design".into()),
                ..Default::default()
            },
            DocumentMeta {
                project: Some("kestrel".into()),
                ..Default::default()
            },
            DocumentMeta {
                created: Some(0),
                ..Default::default()
            },
        ] {
            assert!(!m.is_empty(), "{m:?}");
        }
    }

    /// `created: Some(0)` is a real instant and must not serialize as unset.
    #[test]
    fn a_created_stamp_of_zero_is_not_the_same_as_no_stamp() {
        let epoch = DocumentMeta {
            created: Some(0),
            ..Default::default()
        };
        assert_ne!(epoch, DocumentMeta::default());
        let json = serde_json::to_string(&epoch).unwrap();
        assert_eq!(json, r#"{"created":0}"#);
    }

    /// An empty block writes `{}` and reads back empty, which lets `DocumentDto`
    /// skip it and leave old files byte-identical.
    ///
    /// flip: drop one field's `skip_serializing_if` → fails at the `"{}"` assert
    /// (and the epoch test); `only_the_library_fields_that_were_set_are_written`
    /// in `tests/io.rs` localises it to the key.
    #[test]
    fn an_empty_block_writes_nothing_and_reads_back_empty() {
        let json = serde_json::to_string(&DocumentMeta::default()).unwrap();
        assert_eq!(json, "{}");
        let back: DocumentMeta = serde_json::from_str("{}").unwrap();
        assert!(back.is_empty());
    }
}
