//! Linux's spell checker: Enchant, loaded at run time so a machine without it
//! still runs, unchecked. Enchant fronts Hunspell, Nuspell or Aspell, whichever
//! dictionaries the distribution installed, and keeps the user's own word list.

use super::{Backend, bcp47, closest};
use libloading::Library;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};

type Dict = c_void;
type Broker = c_void;
type Describe =
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char, *mut c_void);

/// Enchant's C API, the calls this module makes.
struct Api {
    broker_free: unsafe extern "C" fn(*mut Broker),
    request_dict: unsafe extern "C" fn(*mut Broker, *const c_char) -> *mut Dict,
    free_dict: unsafe extern "C" fn(*mut Broker, *mut Dict),
    list_dicts: unsafe extern "C" fn(*mut Broker, Describe, *mut c_void),
    check: unsafe extern "C" fn(*mut Dict, *const c_char, isize) -> c_int,
    suggest: unsafe extern "C" fn(*mut Dict, *const c_char, isize, *mut usize) -> *mut *mut c_char,
    free_list: unsafe extern "C" fn(*mut Dict, *mut *mut c_char),
    add: unsafe extern "C" fn(*mut Dict, *const c_char, isize),
}

pub(super) struct Enchant {
    api: Api,
    broker: *mut Broker,
    /// Every installed dictionary as a BCP-47 tag, and as Enchant names it.
    tags: Vec<String>,
    names: Vec<String>,
    dicts: HashMap<String, *mut Dict>,
    /// Declared last so it is dropped last: `api`'s functions live in it.
    _lib: Library,
}

impl Enchant {
    pub(super) fn new() -> Option<Self> {
        // SAFETY: Enchant's own library, whose symbols are declared with the
        // signatures its header gives.
        unsafe {
            let lib = ["libenchant-2.so.2", "libenchant-2.so", "libenchant.so.1"]
                .into_iter()
                .find_map(|n| Library::new(n).ok())?;
            let init: unsafe extern "C" fn() -> *mut Broker =
                *lib.get(b"enchant_broker_init\0").ok()?;
            let api = Api {
                broker_free: *lib.get(b"enchant_broker_free\0").ok()?,
                request_dict: *lib.get(b"enchant_broker_request_dict\0").ok()?,
                free_dict: *lib.get(b"enchant_broker_free_dict\0").ok()?,
                list_dicts: *lib.get(b"enchant_broker_list_dicts\0").ok()?,
                check: *lib.get(b"enchant_dict_check\0").ok()?,
                suggest: *lib.get(b"enchant_dict_suggest\0").ok()?,
                free_list: *lib.get(b"enchant_dict_free_string_list\0").ok()?,
                add: *lib.get(b"enchant_dict_add\0").ok()?,
            };
            let broker = init();
            if broker.is_null() {
                return None;
            }
            let mut names: Vec<String> = Vec::new();
            (api.list_dicts)(broker, describe, (&raw mut names).cast());
            let (tags, names) = names
                .into_iter()
                .filter_map(|n| Some((bcp47(&n)?, n)))
                .unzip();
            Some(Self {
                api,
                broker,
                tags,
                names,
                dicts: HashMap::new(),
                _lib: lib,
            })
        }
    }

    fn dict(&mut self, name: &str) -> Option<*mut Dict> {
        if let Some(d) = self.dicts.get(name) {
            return Some(*d);
        }
        let c = CString::new(name).ok()?;
        // SAFETY: a live broker and a terminated name.
        let d = unsafe { (self.api.request_dict)(self.broker, c.as_ptr()) };
        if d.is_null() {
            return None;
        }
        self.dicts.insert(name.to_string(), d);
        Some(d)
    }
}

/// `enchant_broker_list_dicts`'s callback: push each dictionary's tag onto the
/// `Vec<String>` behind `data`.
unsafe extern "C" fn describe(
    tag: *const c_char,
    _provider: *const c_char,
    _desc: *const c_char,
    _file: *const c_char,
    data: *mut c_void,
) {
    // SAFETY: `data` is the `Vec` `Enchant::new` passed, alive for the call.
    unsafe {
        if tag.is_null() {
            return;
        }
        let names = &mut *data.cast::<Vec<String>>();
        if let Ok(t) = CStr::from_ptr(tag).to_str() {
            names.push(t.to_string());
        }
    }
}

impl Backend for Enchant {
    fn dictionary(&mut self, tag: &str) -> Option<String> {
        let found = closest(tag, &self.tags)?;
        let i = self.tags.iter().position(|t| t == found)?;
        Some(self.names[i].clone())
    }

    /// `LANGUAGE`'s list, then the locale variables, as the C library reads them.
    fn system_tags(&mut self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let language = std::env::var("LANGUAGE").unwrap_or_default();
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|v| std::env::var(v).ok());
        for name in language.split(':').map(str::to_string).chain(locale) {
            if let Some(t) = bcp47(&name)
                && !out.contains(&t)
            {
                out.push(t);
            }
        }
        out
    }

    fn check(&mut self, dict: &str, word: &str) -> bool {
        let Some(d) = self.dict(dict) else {
            return true;
        };
        // SAFETY: a live dictionary and a buffer of the length passed.
        unsafe { (self.api.check)(d, word.as_ptr().cast(), word.len() as isize) == 0 }
    }

    fn suggest(&mut self, dict: &str, word: &str) -> Vec<String> {
        let Some(d) = self.dict(dict) else {
            return Vec::new();
        };
        let mut n = 0usize;
        // SAFETY: the list holds `n` terminated strings and is handed back to
        // Enchant to free.
        unsafe {
            let list = (self.api.suggest)(d, word.as_ptr().cast(), word.len() as isize, &mut n);
            if list.is_null() {
                return Vec::new();
            }
            let out = (0..n)
                .filter_map(|i| CStr::from_ptr(*list.add(i)).to_str().ok())
                .map(str::to_string)
                .collect();
            (self.api.free_list)(d, list);
            out
        }
    }

    fn add(&mut self, dict: &str, word: &str) {
        if let Some(d) = self.dict(dict) {
            // SAFETY: a live dictionary and a buffer of the length passed.
            unsafe { (self.api.add)(d, word.as_ptr().cast(), word.len() as isize) };
        }
    }
}

impl Drop for Enchant {
    fn drop(&mut self) {
        // SAFETY: every dictionary came from this broker, and each is freed once.
        unsafe {
            for d in self.dicts.values() {
                (self.api.free_dict)(self.broker, *d);
            }
            (self.api.broker_free)(self.broker);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On this machine's Enchant: an English word passes, a made-up one is flagged
    /// and gets a suggestion. Skipped where Enchant or an English dictionary is
    /// not installed.
    #[test]
    fn enchant_knows_english() {
        let Some(mut e) = Enchant::new() else {
            eprintln!("no Enchant here; skipped");
            return;
        };
        let Some(en) = e.dictionary("en-US") else {
            eprintln!("no English dictionary here; skipped");
            return;
        };
        assert!(e.check(&en, "house"));
        assert!(e.check(&en, "don't"));
        assert!(!e.check(&en, "hosue"));
        assert!(e.suggest(&en, "hosue").iter().any(|s| s == "house"));
    }
}
