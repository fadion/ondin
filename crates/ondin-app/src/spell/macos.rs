//! macOS's spell checker (`NSSpellChecker`): the system's dictionaries and the
//! user's learned words, which every app shares.

use super::{Backend, bcp47, closest};
use objc2::rc::Retained;
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString};

pub(super) struct MacOs {
    checker: Retained<NSSpellChecker>,
    /// The available languages as BCP-47 tags, and as AppKit names them.
    tags: Vec<String>,
    names: Vec<String>,
}

impl MacOs {
    pub(super) fn new() -> Option<Self> {
        let checker = NSSpellChecker::sharedSpellChecker();
        let (mut tags, mut names) = (Vec::new(), Vec::new());
        for name in checker.availableLanguages().iter() {
            let name = name.to_string();
            if let Some(tag) = bcp47(&name) {
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

impl Backend for MacOs {
    fn dictionary(&mut self, tag: &str) -> Option<String> {
        let found = closest(tag, &self.tags)?;
        let i = self.tags.iter().position(|t| t == found)?;
        Some(self.names[i].clone())
    }

    fn system_tags(&mut self) -> Vec<String> {
        self.checker
            .userPreferredLanguages()
            .iter()
            .filter_map(|n| bcp47(&n.to_string()))
            .collect()
    }

    fn check(&mut self, dict: &str, word: &str) -> bool {
        let s = NSString::from_str(word);
        let lang = NSString::from_str(dict);
        // SAFETY: a null word count is allowed.
        let miss = unsafe {
            self.checker
                .checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
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
