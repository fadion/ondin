//! What a document knows about *itself* as a library entry — the name the user
//! typed for it, which project it belongs to, when it was made (§5.11a, §15
//! D362).
//!
//! **This is the only document state that is not about drawing**, and it lives
//! in the file for one reason: the library's base folder is user-chosen, so it
//! can be a Drive or Dropbox directory, and every fact that has to survive
//! arriving on a second machine has to travel *inside* something that syncs.
//! A cache directory does not sync. So the name, the project and the identity
//! are here, and the facts that are properly local — last opened, thumbnails,
//! the derived index — are not, and must never be moved here for symmetry.
//!
//! ⚠️ **Nothing in here may change as a side effect of editing or saving**,
//! which is the rule that keeps invariant 9 intact. Byte-identical re-saves are
//! what make the format diff well, and one modified-at stamp written by the save
//! path would end that for every document at once. Every field below changes
//! only when the *user* renames the document, moves it to a project, or creates
//! it. If a field you are about to add would be written by a clock, it belongs
//! in the app's cache, not here.
//!
//! **Not an `Operation`, and deliberately outside undo.** Renaming a document is
//! a library action taken from the dashboard — often while the document is not
//! even open — so routing it through `apply` would put an entry in the wrong
//! document's history and make Ctrl+Z in the editor undo something the file
//! browser did. `Document::set_meta` writes it directly, and the dirty flag is
//! the app's business (`crate::document::Document::meta`).

use serde::{Deserialize, Serialize};

/// A document's library identity. Every field is optional, and all-`None` is the
/// state of every file written before this existed.
///
/// **Optional rather than defaulted, because a missing field and a chosen one
/// mean different things here.** An empty name is not "the document is called
/// nothing" — it is "this file predates the library and the app should fall back
/// to its filename". Collapsing the two would make an old document's stem
/// unrecoverable the first time it was saved.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentMeta {
    /// The library-wide identity, minted once and never rewritten.
    ///
    /// **The thing version history and the thumbnail cache are keyed on**, which
    /// is why it exists at all rather than the filename serving as the key: a
    /// rename changes the stem, and history that a rename orphans is history
    /// nobody will trust. It also answers duplication explicitly — *Duplicate*
    /// mints a new one, so a copy is a new document rather than a second file
    /// claiming the first one's past.
    ///
    /// ⚠️ **Minted by the app, never by `io::load`.** A loader that filled this
    /// in would make loading non-deterministic, and reading the same bytes twice
    /// would produce two documents that disagree. So a pre-library file loads
    /// with `None` here and gets its id the first time the library writes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The name the user typed — "My cool design", not `my-cool-design`.
    ///
    /// Stored because the filename cannot give it back: the stem is a lossy
    /// slug, so capitals, spaces and punctuation are gone by the time it reaches
    /// disk (`ondin-app`'s `library::naming`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The id of the project this document belongs to, if any.
    ///
    /// **The grouping is here and not in the folder layout**, so a project is a
    /// set of files that point at it rather than a directory holding them. A
    /// matching folder on disk is optional and the user's choice; moving a file
    /// between folders therefore does not move it between projects, and a file
    /// dragged out of the library entirely still remembers what it belonged to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// When the document was created, in whole seconds since the Unix epoch.
    ///
    /// **Stored rather than read off the filesystem**, because a creation
    /// timestamp is the least portable thing a file has: copying into a synced
    /// folder resets it on Windows, and most Linux filesystems will not report
    /// one at all. The dashboard shows this in a *Created* column, so it has to
    /// be a fact about the document rather than about the inode it currently
    /// occupies.
    ///
    /// Seconds and not a formatted date: no dependency, no timezone to get wrong
    /// on the way in, and the formatting happens where the column is drawn.
    /// Written once at creation — see the module's warning about clocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<u64>,
}

impl DocumentMeta {
    /// Whether nothing has been recorded — the state of every file written
    /// before this block existed.
    ///
    /// **This is what keeps the field purely additive.** `DocumentDto` skips the
    /// whole object when this is true, so a document that has never been through
    /// the library serializes to exactly the bytes it did before, and adding
    /// metadata changed no existing file (§5.11, invariant 9).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Whether `s` has the shape [`Self::id`] is minted in (§15 D419): exactly
    /// [`ID_HEX_LEN`] lowercase hex characters.
    ///
    /// ⚠️ **A security check, not a tidiness one, and this is the only place in
    /// core that knows the shape because the *app* mints it.** The id is joined
    /// as a **path component** by three writers — the crash snapshot, the
    /// version pin and the cover cache — so a `.ondin` whose block reads
    /// `"id": "../my-cool-design"` makes the snapshot writer build
    /// `root/.recovery/../my-cool-design.ondin` and `atomic::write` rename
    /// attacker-chosen bytes over the user's real document, every ten seconds,
    /// silently. On Windows an *absolute* id is worse: `Path::join` discards the
    /// base entirely.
    ///
    /// **The cover cache is the door that needs no click.** It renders every
    /// entry the dashboard's grid draws, so merely opening the library on a base
    /// folder — a folder the design invites the user to point Dropbox at — is
    /// enough. Delivery does not require opening the file.
    ///
    /// The shape is `ondin-app`'s `library::ids::mint`, in plain backticks
    /// because core cannot name it: 16 bytes of `SystemRandom` as `{:02x}`, or a
    /// clock fallback of `f0` plus 30 hex digits. Both are 32 lowercase hex
    /// characters, and nothing else has ever been minted.
    pub fn is_wellformed_id(s: &str) -> bool {
        s.len() == ID_HEX_LEN
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    /// Drop anything in the block that a *file* is not allowed to assert.
    ///
    /// Today that is [`Self::id`] alone, and dropping it rather than refusing the
    /// document is deliberate: `None` is an ordinary, fully supported state —
    /// every pre-library file loads that way — and the library mints a fresh id
    /// the next time it writes. Refusing the load would turn a repairable file
    /// into one that cannot be opened, which is the wrong trade for a field the
    /// drawing does not depend on.
    ///
    /// ⚠️ **`name` and `project` are deliberately *not* touched.** A name is
    /// free text by design and never becomes a path — `library::naming::slug`
    /// stands between it and the filesystem — and a project id is looked up in
    /// `projects.json` rather than joined. The one that needed this is the one
    /// that is used as a path component unexamined.
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
/// Mirrors `ondin-app`'s `library::ids::ID_HEX_LEN`; the two are one number
/// written twice because the minting lives in the app and the *validation* has
/// to be in core, where loading happens.
pub const ID_HEX_LEN: usize = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_meta_is_empty_and_every_field_leaves_it() {
        assert!(DocumentMeta::default().is_empty());
        // Each field on its own is enough to make the block worth writing — the
        // check is `==` against the default rather than four `is_none`s, so this
        // is really asserting that no field was forgotten by that comparison.
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

    /// ⚠️ `created: Some(0)` is the case a `skip_serializing_if =
    /// "is_zero"`-shaped mistake eats, and the epoch is a real instant a clock
    /// skewed to 1970 can produce. Pinned here because the field is the one in
    /// the block whose "unset" and whose "zero" look alike.
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

    /// The round-trip that matters is the *absence* one: an empty block writes
    /// `{}` and reads back empty, which is what lets `DocumentDto` skip it
    /// entirely and leave old files byte-identical.
    ///
    /// Flip-check, run: dropping one field's `skip_serializing_if` fails this
    /// *and* the epoch test above, because both compare a whole serialized
    /// string. Two of the three here bite, which is the cheap kind of redundancy
    /// — but it means neither one localises the mistake to a field. The test
    /// that does is `only_the_library_fields_that_were_set_are_written` over in
    /// `tests/io.rs`, which asserts on the key by name.
    #[test]
    fn an_empty_block_writes_nothing_and_reads_back_empty() {
        let json = serde_json::to_string(&DocumentMeta::default()).unwrap();
        assert_eq!(json, "{}");
        let back: DocumentMeta = serde_json::from_str("{}").unwrap();
        assert!(back.is_empty());
    }
}
