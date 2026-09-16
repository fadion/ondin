//! Reading a document's [`DocumentMeta`] out of the *first few hundred bytes*
//! of its file, without parsing the document (§5.11a, §15 D362).
//!
//! **This exists because listing a folder must not read the documents in it.**
//! The dashboard shows a name, a project and a created date for every file in
//! the library, and all three live in a block `schema.rs` deliberately writes
//! second — but a full `io::load` to reach it would walk the node array and then
//! an image table that is routinely megabytes. Sixty documents is then a
//! multi-second scan of data nothing on screen uses.
//!
//! ⚠️ **And on the setup this whole library is designed for, it is worse than
//! slow.** The base folder is meant to be a Drive, Dropbox or OneDrive
//! directory, where a file that has not been opened on this machine is a
//! *placeholder* — a few bytes of metadata that hydrate into the real thing the
//! moment something reads them. A scan that reads whole documents therefore
//! downloads the entire library to draw a list of names. Reading a fixed 4 KB
//! prefix does not: the placeholder hydrates that much and stops.
//!
//! The scanner below is hand-rolled rather than `serde_json`, for the one reason
//! that matters: serde has to parse a *complete* value, and a prefix is by
//! construction incomplete. What it does instead is walk the top-level object
//! far enough to find one key.

use crate::meta::DocumentMeta;

/// What a prefix was able to say about a document's metadata.
///
/// Three outcomes rather than `Option`, because "there is none" and "I could not
/// tell" lead to different work: the first is an answer the caller can cache,
/// and the second means read the rest of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaProbe {
    /// The block was found and parsed.
    Found(DocumentMeta),
    /// The document definitively carries no metadata.
    ///
    /// ⚠️ **A claim about files this crate wrote**, and the one place this module
    /// trusts key order: `schema.rs` emits `meta` before `root`, so reaching
    /// `root` first means there was none. A hand-edited file that moved the
    /// block below the node array is reported `Absent` when it is not — for
    /// which the cost is the dashboard showing that document's filename instead
    /// of its typed name, and the next save putting the block back where the
    /// writer puts it. Wrong, recoverable, and self-healing; the alternative is
    /// reading every legacy document in the folder in full to prove a negative.
    ///
    /// ⚠️ **That hand-edited file is the *only* wrong answer this variant is
    /// allowed to give, and it used to give three more** (§15 D485). A file that is
    /// not a JSON object at all, a stray byte where a key belongs, and a second
    /// `meta` key all came back `Absent` — so a renamed PNG was reported as a
    /// healthy pre-library document. They are `Inconclusive` now. The distinction
    /// is not academic: the one caller, `library::scan::read_meta`, turns `Absent`
    /// into `Entry::unread = false` without opening the file.
    Absent,
    /// The prefix ran out first — read more, or read the whole file.
    Inconclusive,
}

/// How much of a file is worth reading to answer the question.
///
/// The block is four short fields written second, so a complete one lands well
/// inside 512 bytes; 4 KB is chosen against the *cloud placeholder* rather than
/// against the JSON, since it is also a comfortable minimum read for a hydrating
/// filesystem and leaves room for a future field without a second decision.
pub const PREFIX_BYTES: usize = 4096;

/// Read the metadata block out of the start of a document file.
///
/// `prefix` is the first [`PREFIX_BYTES`] of the file — or the whole file, if it
/// is shorter, which is what lets this return [`MetaProbe::Absent`] for a small
/// document with no block rather than making the caller special-case it.
pub fn meta_in_prefix(prefix: &[u8]) -> MetaProbe {
    let Some(mut i) = skip_ws(prefix, 0) else {
        return MetaProbe::Inconclusive;
    };
    if prefix.get(i) != Some(&b'{') {
        // Not an object at all — a PNG renamed `.ondin`, a text file, a JSON
        // *array*. A truncated read cannot produce this, the first non-space byte
        // of a JSON document being available in any non-empty prefix, so this is a
        // malformed file and the caller's full `load` is the thing that should
        // report it.
        //
        // ⚠️ **That is `Inconclusive`'s contract and it used to answer `Absent`**
        // (§15 D485, `[S1.1-L1-04]`). The two mean opposite things to the one
        // caller that reads them: `library::scan::read_meta` maps `Absent` to a
        // healthy pre-library document with default metadata and only the
        // `Inconclusive` arm reads the file and can report it unreadable. So a
        // corrupt or foreign `.ondin` was told apart from a pre-library one purely
        // by **whether its first non-whitespace byte happened to be `{`** — and the
        // comment right here already said the full load should report it.
        return MetaProbe::Inconclusive;
    }
    i += 1;
    // The last `meta` seen, and whether more than one was.
    //
    // ⚠️ **`serde_json` keeps the *last* duplicate key and this used to return on
    // the first** (§15 D485, `[S1.1-L1-03]`). Both are legal JSON readings; this
    // module's contract is that the two agree, and they key different things — the
    // probe fills `Entry::meta` and `library::cover::key` builds a cover-cache path
    // component from `entry.meta.id`, while the recovery snapshot and the version
    // history use the *loaded* document's. One file was listed, named and
    // cover-cached as document A and opened, autosaved and recovered as document B.
    //
    // Refusing rather than matching serde's rule: a duplicate key is a malformed
    // file, the full load is authoritative anyway, and "keep the last" would still
    // diverge for a second block sitting past `root` or past the prefix.
    let mut found: Option<MetaProbe> = None;
    loop {
        let Some(j) = skip_ws(prefix, i) else {
            return MetaProbe::Inconclusive;
        };
        i = j;
        match prefix.get(i) {
            // An empty or exhausted object: every key seen.
            Some(b'}') => return found.unwrap_or(MetaProbe::Absent),
            Some(b',') => {
                i += 1;
                continue;
            }
            Some(b'"') => {}
            // A byte where a key or a close should be: malformed, not "no
            // metadata" — the same correction as the `{` test above (§15 D485).
            Some(_) => return MetaProbe::Inconclusive,
            None => return MetaProbe::Inconclusive,
        }
        let Some((key, after_key)) = read_string(prefix, i) else {
            return MetaProbe::Inconclusive;
        };
        let Some(colon) = skip_ws(prefix, after_key) else {
            return MetaProbe::Inconclusive;
        };
        if prefix.get(colon) != Some(&b':') {
            return MetaProbe::Inconclusive;
        }
        let Some(value_start) = skip_ws(prefix, colon + 1) else {
            return MetaProbe::Inconclusive;
        };
        let Some(value_end) = scan_value(prefix, value_start) else {
            return MetaProbe::Inconclusive;
        };
        match key.as_str() {
            "meta" if found.is_some() => return MetaProbe::Inconclusive,
            "meta" => {
                found = Some(
                    match serde_json::from_slice::<crate::meta::DocumentMeta>(
                        &prefix[value_start..value_end],
                    ) {
                        // ⚠️ **Sanitized here as well as in `schema::into_document`,
                        // and this is the copy that matters most.** This function is
                        // the *scan's* reader — it never loads the document — so a
                        // `meta.id` read here reaches `library::cover::key` and
                        // becomes a path component **for every card the dashboard
                        // draws**, with the user having opened nothing. The load
                        // path is the door you have to walk through; this is the one
                        // that opens itself.
                        Ok(m) => MetaProbe::Found(m.sanitized()),
                        // Present but unreadable — a future field of a shape this
                        // version cannot deserialize. Not `Absent`: the caller's
                        // full load is the thing entitled to fail loudly.
                        Err(_) => MetaProbe::Inconclusive,
                    },
                );
                i = value_end;
            }
            // Past where the writer puts it. See `MetaProbe::Absent` — and if a
            // block *was* seen, this is where it is finally returned, the scan
            // having carried on to here only to rule out a second one.
            "root" => return found.unwrap_or(MetaProbe::Absent),
            _ => i = value_end,
        }
    }
}

/// The first index at or after `from` that is not JSON whitespace, or `None` if
/// the slice ends first.
fn skip_ws(b: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if !matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Read a JSON string starting at `at` (which must be the opening quote),
/// returning its unescaped-enough contents and the index just past the close.
///
/// **Escapes are skipped, not decoded.** The only strings this reads are
/// top-level *keys*, which are `schema_version`, `meta`, `root` and the like —
/// so the contents only ever need to be compared against ASCII literals, and a
/// `\u` sequence in one means the file is not ours and no key will match anyway.
fn read_string(b: &[u8], at: usize) -> Option<(String, usize)> {
    debug_assert_eq!(b.get(at), Some(&b'"'));
    let mut out = String::new();
    let mut i = at + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                // Consume the escape *and* its payload, so a `\"` does not read
                // as the end of the string.
                out.push('\\');
                i += 2;
            }
            b'"' => return Some((out, i + 1)),
            c => {
                out.push(c as char);
                i += 1;
            }
        }
    }
    None
}

/// The index just past the JSON value starting at `at`, or `None` if the slice
/// ends inside it.
///
/// Depth-counting over objects and arrays, string-aware so a `{` inside a string
/// does not open a level. It never needs to understand a number or a keyword
/// beyond finding where it stops.
fn scan_value(b: &[u8], at: usize) -> Option<usize> {
    let mut i = at;
    let mut depth = 0usize;
    let mut in_string = false;
    while i < b.len() {
        let c = b[i];
        if in_string {
            match c {
                b'\\' => i += 1,
                b'"' => in_string = false,
                _ => {}
            }
            i += 1;
            continue;
        }
        match c {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                // A closer at depth 0 belongs to the *enclosing* object, which
                // means the value was a bare number or keyword and ended just
                // before it.
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b',' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shape `serde_json::to_vec_pretty` produces for a filed
    /// document, trimmed to the keys this module walks. Written out rather than
    /// generated so that a change in `schema.rs`'s key *order* shows up here as
    /// a failing test instead of as a silently slower scan.
    const FILED: &str = r#"{
  "schema_version": 3,
  "meta": {
    "id": "0f9c2b7a4e",
    "name": "My cool design",
    "project": "kestrel",
    "created": 1774483200
  },
  "root": "1a2b:0",
  "nodes": [
    { "id": "1a2b:0" }
  ]
}
"#;

    #[test]
    fn a_filed_document_gives_up_its_block_from_the_prefix() {
        let MetaProbe::Found(m) = meta_in_prefix(FILED.as_bytes()) else {
            panic!("expected the block to be found");
        };
        assert_eq!(m.name.as_deref(), Some("My cool design"));
        assert_eq!(m.project.as_deref(), Some("kestrel"));
        assert_eq!(m.created, Some(1_774_483_200));
    }

    /// ⚠️ The load-bearing case: a prefix that stops **inside the node array**
    /// must still answer, because that is what every real 4 KB read of a real
    /// document looks like.
    #[test]
    fn a_truncated_prefix_still_answers_when_the_block_is_complete() {
        let cut = FILED.find("\"nodes\"").unwrap() + 12;
        let MetaProbe::Found(m) = meta_in_prefix(&FILED.as_bytes()[..cut]) else {
            panic!("expected the block to be found in a truncated prefix");
        };
        assert_eq!(m.name.as_deref(), Some("My cool design"));
    }

    /// And the other side of it: a prefix that stops *before* the block is
    /// complete says so rather than guessing.
    ///
    /// Flip-check, run: making the truncation arms return `Absent` instead
    /// passes every other test in this module and fails only here — which is the
    /// shape that says `Inconclusive` is carrying real weight rather than being
    /// a third case for tidiness.
    #[test]
    fn a_prefix_that_stops_inside_the_block_is_inconclusive() {
        let cut = FILED.find("\"name\"").unwrap() + 8;
        assert_eq!(
            meta_in_prefix(&FILED.as_bytes()[..cut]),
            MetaProbe::Inconclusive
        );
    }

    #[test]
    fn a_pre_library_document_is_absent_once_root_is_reached() {
        let unfiled = r#"{
  "schema_version": 3,
  "root": "1a2b:0",
  "nodes": []
}
"#;
        assert_eq!(meta_in_prefix(unfiled.as_bytes()), MetaProbe::Absent);
    }

    /// A string value before the block must not be able to fake a key or open a
    /// brace — the failure mode a naive `find("\"meta\"")` would have.
    #[test]
    fn a_brace_or_a_key_name_inside_a_string_value_is_not_structure() {
        let tricky = r#"{
  "schema_version": 3,
  "note": "{ \"meta\": {\"name\": \"decoy\"} }",
  "meta": { "name": "real" },
  "root": "1a2b:0"
}
"#;
        let MetaProbe::Found(m) = meta_in_prefix(tricky.as_bytes()) else {
            panic!("expected the real block");
        };
        assert_eq!(m.name.as_deref(), Some("real"));
    }

    /// An empty prefix cannot say anything, and must not say `Absent` — a zero
    /// byte read is what a placeholder that failed to hydrate looks like, and
    /// caching "this document has no name" from it would be permanent.
    #[test]
    fn an_empty_prefix_is_inconclusive() {
        assert_eq!(meta_in_prefix(b""), MetaProbe::Inconclusive);
        assert_eq!(meta_in_prefix(b"  \n"), MetaProbe::Inconclusive);
    }

    /// **A file that is not one of ours is `Inconclusive`, not `Absent`**
    /// (§15 D485, `[S1.1-L1-04]`).
    ///
    /// The two mean opposite things to the one caller that reads them:
    /// `library::scan::read_meta` maps `Absent` to a **healthy pre-library
    /// document** with default metadata, and only the `Inconclusive` arm reads the
    /// file and can report it unreadable. So a corrupt or foreign `.ondin` was told
    /// apart from a pre-library one purely by whether its first non-whitespace byte
    /// happened to be `{` — a PNG, a text file and a JSON *array* all came back as
    /// healthy documents with no metadata.
    ///
    /// **Four shapes, because the byte that decides it is different in each.** A
    /// PNG starts `\x89PNG`; prose starts with a letter; an array starts `[`; and a
    /// stray byte *inside* the object is the fourth arm, which answered `Absent`
    /// for the same wrong reason one level in.
    ///
    /// ⚠️ **The `Absent` cases below are the control and they are the point.** An
    /// empty object and a document whose `meta` sits past `root` still have to
    /// answer `Absent`, or this fix has traded a wrong answer for a slow one:
    /// `Inconclusive` sends every legacy document in the folder through a full
    /// load, which is the cost `Absent` exists to avoid.
    ///
    /// ⚠️ **Flip-check, run: the `!= Some(&b'{')` arm back to `Absent`.** Fails on
    /// the PNG — the first case, and the predicted site — with the array and the
    /// prose behind it. The two `Absent` controls stay green under that flip, which
    /// is what says they are testing the other half.
    #[test]
    fn a_file_that_is_not_ours_is_inconclusive_rather_than_metadata_free() {
        for (what, bytes) in [
            ("a PNG renamed .ondin", &b"\x89PNG\r\n\x1a\n\x00\x00"[..]),
            ("prose renamed .ondin", b"Dear diary, today I"),
            ("a JSON array", b"[1, 2, 3]"),
            ("a stray byte where a key belongs", b"{ 42: 1 }"),
        ] {
            assert_eq!(
                meta_in_prefix(bytes),
                MetaProbe::Inconclusive,
                "{what}: the full load is the thing entitled to report this"
            );
        }

        // The controls: these really are documents with no metadata block, and
        // answering `Inconclusive` for them would send every legacy file in the
        // library through a full load.
        assert_eq!(meta_in_prefix(b"{}"), MetaProbe::Absent);
        assert_eq!(
            meta_in_prefix(br#"{"root": "1a2b:0", "meta": {"name": "late"}}"#),
            MetaProbe::Absent,
            "past where the writer puts it is the one accepted disagreement"
        );
    }

    /// **Two `meta` keys send the caller to the full load** (§15 D485,
    /// `[S1.1-L1-03]`), because the probe and `serde_json` read them in opposite
    /// directions.
    ///
    /// `serde_json` keeps the **last** duplicate; this returned on the **first**.
    /// Both are legal JSON readings, and the module's contract is that the two
    /// agree — they key different things. The probe fills `Entry::meta`, and
    /// `library::cover::key` builds a cover-cache path component out of
    /// `entry.meta.id`, while the recovery snapshot and the version history use the
    /// *loaded* document's. One file was listed, named and cover-cached as document
    /// A and opened, autosaved and recovered as document B.
    ///
    /// **Refusing rather than matching serde's rule**: a duplicate key is a
    /// malformed file, the full load is authoritative anyway, and "keep the last"
    /// would still diverge for a second block sitting past `root` or past the
    /// prefix — so it would fix the fixture below and not the class.
    ///
    /// ⚠️ **The single-`meta` case is asserted beside it**, because the fix defers
    /// the return until `root` in order to see a second key at all: if that walk
    /// were wrong, an ordinary document would answer `Inconclusive` and every card
    /// on the dashboard would cost a full load.
    #[test]
    fn a_duplicate_meta_key_is_inconclusive_rather_than_the_first_one() {
        let two = br#"{"schema_version": 4, "meta": {"name": "first"}, "meta": {"name": "second"}, "root": "1a2b:0"}"#;
        assert_eq!(meta_in_prefix(two), MetaProbe::Inconclusive);

        let one = br#"{"schema_version": 4, "meta": {"name": "only"}, "root": "1a2b:0"}"#;
        match meta_in_prefix(one) {
            MetaProbe::Found(m) => assert_eq!(m.name.as_deref(), Some("only")),
            other => panic!("one block still has to be found, and cheaply: {other:?}"),
        }
    }
}
