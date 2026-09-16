//! The two names a document has, and the rule that turns one into the other.
//!
//! A document is called *My cool design* and lives in `my-cool-design.ondin`.
//! Those are different names for different readers: the first is typed, shown in
//! the dashboard and the top bar, and may contain anything a person can type;
//! the second is what a filesystem, a sync client and `ls` see, and may not.
//! **Neither is derivable from the other** — `slug` is lossy on purpose, so
//! "My cool design", "my cool design" and "My  cool  design!" all land on the
//! same stem — which is why the typed name is stored rather than reconstructed.
//!
//! **The lossiness is the feature and also the whole hazard.** Two documents can
//! legitimately share a typed name, and then they cannot share a file, so
//! [`unique_stem`] is the only function that may name a file: `slug` answers
//! *what is the readable form of this name*, `unique_stem` answers *what may be
//! written into this folder right now*. Anything that writes a path and calls
//! only `slug` is a silent overwrite of somebody else's document.
//!
//! That seam is also where the platform traps live, because both of them are
//! about a name being unavailable rather than about it being unreadable:
//! Windows' reserved device names and a stem already on disk are the same
//! problem and get the same `-1` answer.

/// The longest stem `slug` will emit, in `char`s.
///
/// **A ceiling on the readable part, not on the path.** Windows' real limits are
/// 255 bytes for a component and, without long-path support, 260 for the whole
/// path — and the base folder is user-chosen and can already be deep, so a stem
/// that merely fits in a component can still fail to be created. Sixty leaves
/// room for the `-12` a collision adds, the `.ondin` extension, and the
/// `.versions/<stem>/<stamp>.ondin` that version history nests underneath the
/// same root — which is the longest path this library builds and therefore the
/// one the number has to be chosen against.
///
/// It is also about reading: a folder the user is meant to be able to open in
/// Explorer and recognise is one where the names fit in a column.
const MAX_STEM_CHARS: usize = 60;

/// The stem used when a typed name has nothing a filename can keep.
///
/// Reachable from a name that is not empty — "?!" and "…" both slug to nothing —
/// so this is not the same case as an unnamed document and must not be assumed
/// to be rare.
const FALLBACK_STEM: &str = "untitled";

/// Windows device names, which are unavailable **with any extension**: `nul.ondin`
/// opens the null device rather than a file, and the error a create returns for
/// it does not say so.
///
/// Kept lowercase because `slug` has already lowercased its output, so the
/// comparison is a plain `contains` rather than a case fold. `COM0`/`LPT0` are
/// included: they are not reserved on every Windows version, and a stem this
/// list refuses costs one `-1` suffix, while a stem it wrongly allows costs a
/// document that cannot be saved.
const RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The readable filename stem for a typed document name — lowercase, words
/// joined by `-`, nothing else.
///
/// **Alphanumeric is asked of Unicode, not of ASCII**, so "Café" keeps its `é`
/// and "設計案" keeps all three characters instead of collapsing to a row of
/// dashes. Every filesystem this app targets stores UTF-8 names, and folding to
/// ASCII would make two visibly different documents share a stem for no gain.
/// Everything else — spaces, punctuation, emoji, and each of Windows'
/// `\ / : * ? " < > |` — becomes a separator, so the illegal characters are
/// excluded by the rule that is already there rather than by a second list that
/// could disagree with it.
///
/// Runs of separators collapse and the ends are trimmed, which is also what
/// keeps the result off two more Windows rules: a name may not end in a `.` or a
/// space, and neither survives being a separator that got trimmed.
///
/// Returns [`FALLBACK_STEM`] rather than an empty string, because every caller
/// would otherwise have to repeat that check and one of them would not.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut len = 0usize;
    let mut pending_sep = false;
    for ch in name.chars() {
        if !ch.is_alphanumeric() {
            pending_sep = true;
            continue;
        }
        // `to_lowercase` rather than `to_ascii_lowercase`: one char can lowercase
        // to several (İ → i̇), so the width this costs is not always 1 and the
        // cap has to ask rather than assume.
        let lower: String = ch.to_lowercase().collect();
        let sep = usize::from(pending_sep && !out.is_empty());
        // ⚠️ **Measured before the write, not after.** Breaking once the string
        // is already too long is the version that reads correctly and emits a
        // stem one separator and one letter over the cap — which is what the
        // first draft of this did, and what `a_long_name_is_capped…` failed on.
        if len + sep + lower.chars().count() > MAX_STEM_CHARS {
            break;
        }
        if sep == 1 {
            out.push('-');
        }
        out.push_str(&lower);
        len += sep + lower.chars().count();
        pending_sep = false;
    }
    // The cap can land mid-word but never on a `-`, since a separator is only
    // ever written together with the character that follows it — so no trailing
    // trim is owed here.
    if out.is_empty() {
        FALLBACK_STEM.to_string()
    } else {
        out
    }
}

/// The stem a file may actually be created under: [`slug`], then `-1`, `-2`, …
/// until `taken` says no.
///
/// `taken` is a predicate rather than a directory listing because the two
/// callers ask it differently — creating a document asks the filesystem, and the
/// dashboard's rename field asks it *excluding the file being renamed*, or every
/// rename that does not change the name would walk one suffix further.
///
/// ⚠️ **A reserved stem is fed through the same loop as a collision**, so `NUL`
/// becomes `nul-1` — `nul` is refused for a reason the caller cannot see and a
/// suffix is the answer that needs no new vocabulary. Only the bare stem is
/// checked: `nul-1` is an ordinary filename, and the loop can therefore always
/// terminate.
///
/// The suffix is appended to the *slug*, not to the typed name, so renaming
/// "My cool design" into a folder that already has one yields
/// `my-cool-design-1.ondin` while the document is still called "My cool design".
/// Two documents sharing a typed name is legal here (§15) and only the disk
/// disambiguates.
pub fn unique_stem(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base = slug(name);
    if !RESERVED_STEMS.contains(&base.as_str()) && !taken(&base) {
        return base;
    }
    // The cap is on the *slug*, so the suffix may push the stem a few chars past
    // `MAX_STEM_CHARS`. Deliberate: truncating the base to make room would make
    // `-9` and `-10` name different documents, which is the one bug this loop
    // exists to prevent.
    for n in 1u32.. {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    unreachable!("the suffix range is exhausted only after 4 billion collisions")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A predicate over a fixed set, which is what both real callers reduce to.
    fn taken_of(names: &[&str]) -> impl Fn(&str) -> bool + use<> {
        let set: HashSet<String> = names.iter().map(|s| s.to_string()).collect();
        move |s: &str| set.contains(s)
    }

    #[test]
    fn the_typed_name_becomes_a_lowercase_dashed_stem() {
        assert_eq!(slug("My cool design"), "my-cool-design");
        assert_eq!(slug("Landing v4"), "landing-v4");
        // Runs collapse and the ends are trimmed — the three cases that would
        // otherwise emit `--`, a leading `-` and a trailing `-` respectively.
        assert_eq!(slug("My  cool   design"), "my-cool-design");
        assert_eq!(slug("  Hero  "), "hero");
        assert_eq!(slug("...Hero..."), "hero");
    }

    /// ⚠️ **The illegal-character set is covered by the alphanumeric rule, not by
    /// a list.** Flipping the rule to an explicit blocklist is exactly the
    /// implementation somebody would write instead, and it passes the case above
    /// while letting `:` through on the one platform that cares.
    #[test]
    fn every_character_windows_forbids_becomes_a_separator() {
        assert_eq!(slug(r#"a\b/c:d*e?f"g<h>i|j"#), "a-b-c-d-e-f-g-h-i-j");
        // A trailing dot and a trailing space are both illegal on Windows and
        // both are separators here, so the trim is what removes them.
        assert_eq!(slug("Report."), "report");
        assert_eq!(slug("Report "), "report");
    }

    #[test]
    fn non_ascii_letters_survive_and_are_lowercased() {
        assert_eq!(slug("Café Menu"), "café-menu");
        assert_eq!(slug("設計案"), "設計案");
        // Emoji are not alphanumeric, so they separate rather than vanish
        // silently into the middle of a word.
        assert_eq!(slug("Ship 🚀 it"), "ship-it");
    }

    /// A name with nothing keepable in it is *not* the same case as an unnamed
    /// document, and the fallback is what stops it becoming an empty path.
    #[test]
    fn a_name_with_no_keepable_characters_falls_back() {
        assert_eq!(slug("?!"), FALLBACK_STEM);
        assert_eq!(slug("…"), FALLBACK_STEM);
        assert_eq!(slug(""), FALLBACK_STEM);
    }

    /// The cap counts `char`s, and — the half worth pinning — it can never leave
    /// a trailing `-`, because a separator is only ever written together with
    /// the character that follows it.
    ///
    /// ⚠️ **This is the test that caught the cap being checked after the write
    /// rather than before**, which emitted 61 characters for the first fixture:
    /// the separator and the letter that crossed the line had both already been
    /// pushed by the time the loop looked. Two fixtures because they fail
    /// differently — the many-word one goes *over* the cap, and the single-word
    /// one is what pins the boundary to exactly `MAX_STEM_CHARS` rather than to
    /// "somewhere near it".
    #[test]
    fn a_long_name_is_capped_and_never_ends_in_a_dash() {
        let many_words = slug(&"word ".repeat(40));
        assert!(many_words.chars().count() <= MAX_STEM_CHARS, "{many_words}");
        assert!(!many_words.ends_with('-'), "{many_words}");
        // A word boundary rarely lands on the cap, so this one stops short of it
        // — the assertion is that it stops *under*, not that it fills.
        assert_eq!(many_words.chars().count(), 59);

        // One long word has no separators to stop early on, so it is the fixture
        // that reaches the cap exactly.
        let one_word = slug(&"a".repeat(200));
        assert_eq!(one_word.chars().count(), MAX_STEM_CHARS);
    }

    #[test]
    fn a_free_stem_is_used_unchanged() {
        assert_eq!(
            unique_stem("My cool design", &taken_of(&[])),
            "my-cool-design"
        );
    }

    #[test]
    fn a_collision_walks_the_suffix_until_the_folder_is_quiet() {
        let taken = taken_of(&["my-cool-design", "my-cool-design-1", "my-cool-design-2"]);
        assert_eq!(unique_stem("My cool design", &taken), "my-cool-design-3");
    }

    /// ⚠️ Windows' device names are unavailable *with* an extension, so this is
    /// the one rule that cannot be checked by trying the create and reading the
    /// error — the error does not say the name was the problem.
    ///
    /// Flip-check: dropping the `RESERVED_STEMS` arm from `unique_stem` fails
    /// here on `nul` and leaves every other test in this module green, which is
    /// the shape that says the guard is load-bearing and unshared.
    #[test]
    fn a_windows_device_name_is_suffixed_even_in_an_empty_folder() {
        let empty = taken_of(&[]);
        assert_eq!(unique_stem("NUL", &empty), "nul-1");
        assert_eq!(unique_stem("com1", &empty), "com1-1");
        assert_eq!(unique_stem("Aux", &empty), "aux-1");
        // Only the bare stem is reserved. `nul-1` is an ordinary name, which is
        // what makes the loop terminate rather than walking forever.
        assert_eq!(unique_stem("nul-1", &empty), "nul-1");
        // And a device name is only a device name whole: `nullify` is fine.
        assert_eq!(unique_stem("nullify", &empty), "nullify");
    }

    /// The suffix goes on the slug, and the typed name is untouched — the
    /// property the dashboard depends on to show two documents both called
    /// "My cool design".
    #[test]
    fn two_documents_may_share_a_typed_name() {
        let first = unique_stem("My cool design", &taken_of(&[]));
        let second = unique_stem("My cool design", &taken_of(&[&first]));
        assert_ne!(first, second);
        assert_eq!(
            (first.as_str(), second.as_str()),
            ("my-cool-design", "my-cool-design-1")
        );
    }
}
