//! Reading a document's [`DocumentMeta`] out of the *first few hundred bytes*
//! of its file, without parsing the document (§5.11a, §15 D362).
//!
//! Listing a folder must not read the documents in it. The name, project and
//! created date live in a block `schema.rs` writes second, so a prefix reaches
//! them without walking the node array and image table. The base folder is meant
//! to be a Drive, Dropbox or OneDrive directory, where an unopened file is a
//! placeholder that hydrates on read; a fixed 4 KB prefix hydrates only that much.
//!
//! The scanner is hand-rolled rather than `serde_json` because serde needs a
//! complete value and a prefix is incomplete: it walks the top-level object far
//! enough to find one key.

use crate::meta::DocumentMeta;

/// What a prefix was able to say about a document's metadata.
///
/// "There is none" is an answer the caller can cache; "I could not tell" means
/// read the rest of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetaProbe {
    /// The block was found and parsed.
    Found(DocumentMeta),
    /// The document definitively carries no metadata.
    ///
    /// The one place this module trusts key order: `schema.rs` emits `meta` before
    /// `root`, so reaching `root` first means there was none. A hand-edited file
    /// that moved the block lower is wrongly `Absent`; the cost is the filename
    /// shown instead of the typed name until the next save. That is the only
    /// wrong answer allowed (§15 D485): a non-object file, a stray byte where a
    /// key belongs and a second `meta` key are `Inconclusive`, because
    /// `library::scan::read_meta` turns `Absent` into `Entry::unread = false`
    /// without opening the file.
    Absent,
    /// The prefix ran out first — read more, or read the whole file.
    Inconclusive,
}

/// How much of a file is worth reading to answer the question.
///
/// The block lands well inside 512 bytes; 4 KB is chosen against the cloud
/// placeholder, as a comfortable minimum read for a hydrating filesystem.
pub const PREFIX_BYTES: usize = 4096;

/// Read the metadata block out of the start of a document file.
///
/// `prefix` is the first [`PREFIX_BYTES`] of the file, or the whole file if
/// shorter, so a small document with no block is [`MetaProbe::Absent`].
pub fn meta_in_prefix(prefix: &[u8]) -> MetaProbe {
    let Some(mut i) = skip_ws(prefix, 0) else {
        return MetaProbe::Inconclusive;
    };
    if prefix.get(i) != Some(&b'{') {
        // Not an object (a renamed PNG, text, a JSON array). A truncated read
        // cannot produce this, so the file is malformed and the full `load` should
        // report it; `Absent` would pass it off as a healthy pre-library document
        // (§15 D485).
        return MetaProbe::Inconclusive;
    }
    i += 1;
    // The `meta` seen so far. A duplicate key is refused rather than resolved the
    // way `serde_json` does (last wins): the probe and the loaded document would
    // otherwise name and cover-cache a file as different documents, and "keep the
    // last" would still diverge for a second block past `root` or the prefix
    // (§15 D485).
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
            // A byte where a key or a close should be: malformed, not "no metadata".
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
                        // Sanitized here as well as in `schema::into_document`: this
                        // is the scan's reader, and `meta.id` becomes a path
                        // component in `library::cover::key` for every card drawn,
                        // with no document opened.
                        Ok(m) => MetaProbe::Found(m.sanitized()),
                        // Present but unreadable (a future field of a shape this
                        // version cannot deserialize): the full load fails loudly.
                        Err(_) => MetaProbe::Inconclusive,
                    },
                );
                i = value_end;
            }
            // Past where the writer puts it (see `MetaProbe::Absent`); a block seen
            // earlier is returned here, the scan having only ruled out a second.
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
/// Escapes are skipped, not decoded: only top-level keys are read, compared
/// against ASCII literals, and a `\u` in one means no key will match anyway.
fn read_string(b: &[u8], at: usize) -> Option<(String, usize)> {
    debug_assert_eq!(b.get(at), Some(&b'"'));
    let mut out = String::new();
    let mut i = at + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                // Consume the payload too, so `\"` does not end the string.
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
/// does not open a level.
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
                // A closer at depth 0 belongs to the enclosing object: the value
                // was a bare number or keyword.
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

    /// A filed document as `serde_json::to_vec_pretty` writes it, trimmed to the
    /// keys this module walks; written out so a change in `schema.rs`'s key order
    /// fails a test.
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

    /// A prefix that stops inside the node array still answers; that is what every
    /// real 4 KB read looks like.
    #[test]
    fn a_truncated_prefix_still_answers_when_the_block_is_complete() {
        let cut = FILED.find("\"nodes\"").unwrap() + 12;
        let MetaProbe::Found(m) = meta_in_prefix(&FILED.as_bytes()[..cut]) else {
            panic!("expected the block to be found in a truncated prefix");
        };
        assert_eq!(m.name.as_deref(), Some("My cool design"));
    }

    /// A prefix that stops before the block is complete says so rather than guessing.
    ///
    /// flip: truncation arms returning `Absent` → fails here only.
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

    /// A string value before the block cannot fake a key or open a brace.
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

    /// An empty prefix must not say `Absent`: a zero-byte read is a placeholder
    /// that failed to hydrate, and caching "no name" from it would stick.
    #[test]
    fn an_empty_prefix_is_inconclusive() {
        assert_eq!(meta_in_prefix(b""), MetaProbe::Inconclusive);
        assert_eq!(meta_in_prefix(b"  \n"), MetaProbe::Inconclusive);
    }

    /// A file that is not one of ours is `Inconclusive`, not `Absent` (§15 D485):
    /// `Absent` reads as a healthy pre-library document. The four shapes differ in
    /// the deciding byte; the `Absent` controls (an empty object, `meta` past
    /// `root`) must stay `Absent`, or every legacy document costs a full load.
    ///
    /// flip: the `!= Some(&b'{')` arm answering `Absent` → fails at the PNG case;
    /// the controls stay green.
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

        // The controls: these really have no metadata block.
        assert_eq!(meta_in_prefix(b"{}"), MetaProbe::Absent);
        assert_eq!(
            meta_in_prefix(br#"{"root": "1a2b:0", "meta": {"name": "late"}}"#),
            MetaProbe::Absent,
            "past where the writer puts it is the one accepted disagreement"
        );
    }

    /// Two `meta` keys send the caller to the full load (§15 D485), since the
    /// probe and `serde_json` would pick different ones. The single-`meta` case
    /// beside it checks that deferring the return until `root` still finds an
    /// ordinary block.
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
