//! The two names a document has, and the rule that turns one into the other.
//!
//! A document is called *My cool design* and lives in `my-cool-design.ondin`: the
//! typed name is shown in the UI, the stem is what the filesystem sees. `slug` is
//! lossy, so the typed name is stored, and [`unique_stem`] is the only function
//! that may name a file (`slug` alone would silently overwrite another document).
//! Windows' reserved device names and a stem already on disk get the same `-1` answer.

/// The longest stem `slug` will emit, in `char`s.
///
/// A ceiling on the readable part, not the path: the base folder is user-chosen and
/// Windows limits a path to 260 without long-path support. Sixty leaves room for the
/// collision suffix, the extension and the `.versions/<stem>/<stamp>.ondin` nesting,
/// the longest path this library builds.
const MAX_STEM_CHARS: usize = 60;

/// The stem used when a typed name has nothing a filename can keep ("?!" and "…"
/// both slug to nothing, so it is not rare).
const FALLBACK_STEM: &str = "untitled";

/// Windows device names, unavailable with any extension (`nul.ondin` opens the null
/// device and the create error does not say so).
///
/// Lowercase because `slug` has lowercased its output. `COM0`/`LPT0` are included:
/// refusing a stem costs one `-1`, wrongly allowing one costs an unsavable document.
const RESERVED_STEMS: &[&str] = &[
    "con", "prn", "aux", "nul", "com0", "com1", "com2", "com3", "com4", "com5", "com6", "com7",
    "com8", "com9", "lpt0", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The readable filename stem for a typed document name — lowercase, words
/// joined by `-`, nothing else.
///
/// Alphanumeric is asked of Unicode, so "Café" and "設計案" keep their letters.
/// Everything else, including each of Windows' `\ / : * ? " < > |`, becomes a
/// separator, so no second blocklist can disagree. Runs collapse and the ends are
/// trimmed, which also keeps a trailing `.` or space out. Returns [`FALLBACK_STEM`]
/// rather than an empty string.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut len = 0usize;
    let mut pending_sep = false;
    for ch in name.chars() {
        if !ch.is_alphanumeric() {
            pending_sep = true;
            continue;
        }
        // One char can lowercase to several (İ → i̇), so the cap measures the result.
        let lower: String = ch.to_lowercase().collect();
        let sep = usize::from(pending_sep && !out.is_empty());
        // Measured before the write: checking afterwards overshoots the cap by a
        // separator and a letter.
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
    // A separator is written only with the character after it, so no trailing trim.
    if out.is_empty() {
        FALLBACK_STEM.to_string()
    } else {
        out
    }
}

/// The stem a file may actually be created under: [`slug`], then `-1`, `-2`, …
/// until `taken` says no.
///
/// `taken` is a predicate because the callers differ: creation asks the filesystem,
/// the dashboard's rename field asks it excluding the file being renamed. A reserved
/// stem goes through the same loop (`NUL` becomes `nul-1`); only the bare stem is
/// checked, so the loop terminates. The suffix goes on the slug, not the typed name,
/// so two documents may share a typed name (§15) and only the disk disambiguates.
pub fn unique_stem(name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let base = slug(name);
    if !RESERVED_STEMS.contains(&base.as_str()) && !taken(&base) {
        return base;
    }
    // The suffix may push the stem past `MAX_STEM_CHARS`; truncating the base to make
    // room would make `-9` and `-10` collide.
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

    /// A predicate over a fixed set.
    fn taken_of(names: &[&str]) -> impl Fn(&str) -> bool + use<> {
        let set: HashSet<String> = names.iter().map(|s| s.to_string()).collect();
        move |s: &str| set.contains(s)
    }

    #[test]
    fn the_typed_name_becomes_a_lowercase_dashed_stem() {
        assert_eq!(slug("My cool design"), "my-cool-design");
        assert_eq!(slug("Landing v4"), "landing-v4");
        // Runs collapse and the ends are trimmed.
        assert_eq!(slug("My  cool   design"), "my-cool-design");
        assert_eq!(slug("  Hero  "), "hero");
        assert_eq!(slug("...Hero..."), "hero");
    }

    /// The illegal-character set is covered by the alphanumeric rule, not a list.
    #[test]
    fn every_character_windows_forbids_becomes_a_separator() {
        assert_eq!(slug(r#"a\b/c:d*e?f"g<h>i|j"#), "a-b-c-d-e-f-g-h-i-j");
        // A trailing dot or space is illegal on Windows; the trim removes it.
        assert_eq!(slug("Report."), "report");
        assert_eq!(slug("Report "), "report");
    }

    #[test]
    fn non_ascii_letters_survive_and_are_lowercased() {
        assert_eq!(slug("Café Menu"), "café-menu");
        assert_eq!(slug("設計案"), "設計案");
        // Emoji separate rather than vanish into a word.
        assert_eq!(slug("Ship 🚀 it"), "ship-it");
    }

    /// A name with nothing keepable falls back rather than becoming an empty path.
    #[test]
    fn a_name_with_no_keepable_characters_falls_back() {
        assert_eq!(slug("?!"), FALLBACK_STEM);
        assert_eq!(slug("…"), FALLBACK_STEM);
        assert_eq!(slug(""), FALLBACK_STEM);
    }

    /// The cap counts `char`s and never leaves a trailing `-`. The many-word fixture
    /// catches a check made after the write; the single word pins the exact boundary.
    #[test]
    fn a_long_name_is_capped_and_never_ends_in_a_dash() {
        let many_words = slug(&"word ".repeat(40));
        assert!(many_words.chars().count() <= MAX_STEM_CHARS, "{many_words}");
        assert!(!many_words.ends_with('-'), "{many_words}");
        // A word boundary rarely lands on the cap; this stops under it.
        assert_eq!(many_words.chars().count(), 59);

        // One long word reaches the cap exactly.
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

    /// Windows device names are suffixed even in an empty folder; the create error
    /// would not name them as the cause.
    ///
    /// flip: drop the `RESERVED_STEMS` arm from `unique_stem` → fails at the `nul` assertion.
    #[test]
    fn a_windows_device_name_is_suffixed_even_in_an_empty_folder() {
        let empty = taken_of(&[]);
        assert_eq!(unique_stem("NUL", &empty), "nul-1");
        assert_eq!(unique_stem("com1", &empty), "com1-1");
        assert_eq!(unique_stem("Aux", &empty), "aux-1");
        // Only the bare stem is reserved, which lets the loop terminate.
        assert_eq!(unique_stem("nul-1", &empty), "nul-1");
        // A device name is only a device name whole.
        assert_eq!(unique_stem("nullify", &empty), "nullify");
    }

    /// The suffix goes on the slug, so two documents can both be called "My cool design".
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
