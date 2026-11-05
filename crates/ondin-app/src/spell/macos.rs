//! macOS's spell checker (`NSSpellChecker`): the system's dictionaries and the
//! user's learned words, which every app shares.

use super::{Backend, bcp47, closest};
use objc2::rc::Retained;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSLocale, NSRange, NSString};

/// A word no dictionary holds. A listed language that accepts it checks nothing
/// (some, Greek and Arabic among them, pass any word, their own script's
/// nonsense included) and would hide every typo in a union.
const NONSENSE: &str = "qzxqzxqzx";

pub(super) struct MacOs {
    checker: Retained<NSSpellChecker>,
    /// The available languages that check, as BCP-47 tags and as AppKit names them.
    tags: Vec<String>,
    names: Vec<String>,
}

impl MacOs {
    pub(super) fn new() -> Option<Self> {
        let checker = NSSpellChecker::sharedSpellChecker();
        let (mut tags, mut names) = (Vec::new(), Vec::new());
        for name in checker.availableLanguages().iter() {
            let name = name.to_string();
            if let Some(tag) = bcp47(&name)
                && !accepts(&checker, &name, NONSENSE)
            {
                tags.push(tag);
                names.push(name);
            }
        }
        Some(Self {
            checker,
            tags,
            names,
        })
    }
}

/// Whether `dict` finds no misspelling in `word`.
fn accepts(checker: &NSSpellChecker, dict: &str, word: &str) -> bool {
    let s = NSString::from_str(word);
    let lang = NSString::from_str(dict);
    // SAFETY: a null word count is allowed.
    let miss = unsafe {
        checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
            &s,
            0,
            Some(&lang),
            false,
            0,
            std::ptr::null_mut(),
        )
    };
    miss.length == 0
}

impl Backend for MacOs {
    fn dictionary(&mut self, tag: &str) -> Option<String> {
        let found = closest(tag, &self.tags)?;
        let i = self.tags.iter().position(|t| t == found)?;
        Some(self.names[i].clone())
    }

    /// The user's languages (System Settings › Language & Region), as the
    /// keyboard languages are on Windows. Not the checker's
    /// `userPreferredLanguages`: that is every language its panel enables, which
    /// checked together accept everything. The checker's own language when the
    /// list is empty.
    fn system_tags(&mut self) -> Vec<String> {
        let tags: Vec<String> = NSLocale::preferredLanguages()
            .iter()
            .filter_map(|n| bcp47(&n.to_string()))
            .collect();
        if tags.is_empty() {
            bcp47(&self.checker.language().to_string())
                .into_iter()
                .collect()
        } else {
            tags
        }
    }

    fn check(&mut self, dict: &str, word: &str) -> bool {
        accepts(&self.checker, dict, word)
    }

    fn suggest(&mut self, dict: &str, word: &str) -> Vec<String> {
        let s = NSString::from_str(word);
        let lang = NSString::from_str(dict);
        self.checker
            .guessesForWordRange_inString_language_inSpellDocumentWithTag(
                NSRange::new(0, s.length()),
                &s,
                Some(&lang),
                0,
            )
            .map(|a| a.iter().map(|g| g.to_string()).collect())
            .unwrap_or_default()
    }

    /// Learned words are not per language on macOS.
    fn add(&mut self, _dict: &str, word: &str) {
        self.checker.learnWord(&NSString::from_str(word));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Untagged text, checked as the app checks it — in every dictionary of the
    /// user's languages, passing if any accepts — flags a typo and passes a word,
    /// on the machine's own `NSSpellChecker`.
    ///
    /// Two defences hold this, and on a machine with a dictionary that passes any
    /// word either alone is enough, so the flip removes both.
    ///
    /// flip: `system_tags` back on `userPreferredLanguages` and `MacOs::new`
    /// keeping every language → fails at "teh is flagged".
    #[test]
    fn untagged_text_is_checked_in_the_users_languages() {
        let mut b = MacOs::new().expect("the macOS checker");
        let mut cache = std::collections::HashMap::new();
        let dicts = super::super::dictionaries(&mut b, &mut cache, &None);
        assert!(
            !dicts.is_empty(),
            "fixture: a dictionary for the user's languages"
        );
        let passes = |b: &mut MacOs, w: &str| dicts.iter().any(|d| b.check(d, w));
        assert!(!passes(&mut b, "teh"), "teh is flagged, in {dicts:?}");
        assert!(passes(&mut b, "hello"), "hello passes, in {dicts:?}");
    }

    /// No language the backend keeps passes a word no dictionary holds.
    ///
    /// flip: `MacOs::new` keeping every language → fails at "checks nothing",
    /// only on a Mac whose checker lists a dead dictionary.
    #[test]
    fn a_dictionary_that_passes_any_word_is_left_out() {
        let b = MacOs::new().expect("the macOS checker");
        let dead: Vec<&String> = b
            .names
            .iter()
            .filter(|n| accepts(&b.checker, n, NONSENSE))
            .collect();
        assert!(dead.is_empty(), "{dead:?} checks nothing");
    }
}
