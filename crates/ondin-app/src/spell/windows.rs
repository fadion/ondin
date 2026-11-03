//! Windows' spell checker (`ISpellCheckerFactory`): the dictionaries of the
//! language packs the user installed, and the user's own word list, which Word
//! and Edge share.

use super::{Backend, closest};
use std::collections::HashMap;
use windows::Win32::Globalization::{
    CORRECTIVE_ACTION_NONE, GetUserDefaultLocaleName, ISpellChecker, ISpellCheckerFactory,
    LCIDToLocaleName, SpellCheckerFactory,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    IEnumString,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayoutList, HKL};
use windows::core::{HSTRING, PWSTR};

/// `LOCALE_NAME_MAX_LENGTH`, in UTF-16 units with the terminator.
const LOCALE_NAME_LEN: usize = 85;

pub(super) struct Windows {
    factory: ISpellCheckerFactory,
    /// The languages a checker can be made for, as BCP-47 tags.
    supported: Vec<String>,
    checkers: HashMap<String, ISpellChecker>,
}

impl Windows {
    /// The factory on this thread, or `None` where Windows has no checker.
    pub(super) fn new() -> Option<Self> {
        // SAFETY: plain COM calls on the worker thread, which joins the
        // multithreaded apartment here; an error means it already belongs to one.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let factory: ISpellCheckerFactory =
                CoCreateInstance(&SpellCheckerFactory, None, CLSCTX_INPROC_SERVER).ok()?;
            let supported = strings(&factory.SupportedLanguages().ok()?);
            Some(Self {
                factory,
                supported,
                checkers: HashMap::new(),
            })
        }
    }

    fn checker(&mut self, dict: &str) -> Option<&ISpellChecker> {
        if !self.checkers.contains_key(dict) {
            // SAFETY: a COM call with a valid string.
            let c = unsafe { self.factory.CreateSpellChecker(&HSTRING::from(dict)) }.ok()?;
            self.checkers.insert(dict.to_string(), c);
        }
        self.checkers.get(dict)
    }
}

impl Backend for Windows {
    fn dictionary(&mut self, tag: &str) -> Option<String> {
        closest(tag, &self.supported).cloned()
    }

    /// The languages of the keyboard layouts installed, else the user's locale.
    fn system_tags(&mut self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        // SAFETY: the buffers are sized by the first call and each name is
        // read up to the length the call reports.
        unsafe {
            let n = GetKeyboardLayoutList(None).max(0) as usize;
            let mut layouts = vec![HKL::default(); n];
            let n = GetKeyboardLayoutList(Some(&mut layouts)).max(0) as usize;
            for hkl in &layouts[..n.min(layouts.len())] {
                // The low word of a layout handle is its language id.
                let lang = (hkl.0 as usize & 0xFFFF) as u32;
                let mut buf = [0u16; LOCALE_NAME_LEN];
                let len = LCIDToLocaleName(lang, Some(&mut buf), 0);
                if len > 1 {
                    let name = String::from_utf16_lossy(&buf[..len as usize - 1]);
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
            if out.is_empty() {
                let mut buf = [0u16; LOCALE_NAME_LEN];
                let len = GetUserDefaultLocaleName(&mut buf);
                if len > 1 {
                    out.push(String::from_utf16_lossy(&buf[..len as usize - 1]));
                }
            }
        }
        out
    }

    fn check(&mut self, dict: &str, word: &str) -> bool {
        let Some(c) = self.checker(dict) else {
            return true;
        };
        // SAFETY: COM calls on interfaces this thread made.
        unsafe {
            let Ok(errors) = c.Check(&HSTRING::from(word)) else {
                return true;
            };
            let mut error = None;
            // `S_FALSE` and no error: the checker has nothing to say about it.
            if errors.Next(&mut error).is_err() {
                return true;
            }
            error.is_none_or(|e| {
                e.CorrectiveAction()
                    .is_ok_and(|a| a == CORRECTIVE_ACTION_NONE)
            })
        }
    }

    fn suggest(&mut self, dict: &str, word: &str) -> Vec<String> {
        let Some(c) = self.checker(dict) else {
            return Vec::new();
        };
        // SAFETY: a COM call on an interface this thread made.
        unsafe { c.Suggest(&HSTRING::from(word)) }
            .map(|e| strings(&e))
            .unwrap_or_default()
    }

    fn add(&mut self, dict: &str, word: &str) {
        if let Some(c) = self.checker(dict) {
            // SAFETY: a COM call on an interface this thread made.
            let _ = unsafe { c.Add(&HSTRING::from(word)) };
        }
    }
}

/// Every string of a COM enumerator, each freed once read.
fn strings(e: &IEnumString) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let mut item = [PWSTR::null()];
        let mut fetched = 0u32;
        // SAFETY: one slot, whose string the enumerator allocates and this
        // function frees with the allocator COM names.
        unsafe {
            if e.Next(&mut item, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            if let Ok(s) = item[0].to_string() {
                out.push(s);
            }
            CoTaskMemFree(Some(item[0].0 as *const _));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On this machine's real checker: an English word passes, a made-up one is
    /// flagged and gets a suggestion. Skipped where no English dictionary is
    /// installed, which a Windows runner normally has.
    #[test]
    fn the_windows_checker_knows_english() {
        let mut w = Windows::new().expect("Windows 8 and later have the factory");
        let Some(en) = w.dictionary("en-US") else {
            eprintln!("no English dictionary here; skipped");
            return;
        };
        assert!(w.check(&en, "house"));
        assert!(w.check(&en, "don't"));
        assert!(!w.check(&en, "hosue"));
        assert!(w.suggest(&en, "hosue").iter().any(|s| s == "house"));
        assert!(
            !w.system_tags().is_empty(),
            "some input language is installed"
        );
    }
}
