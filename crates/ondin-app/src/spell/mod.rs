//! Spell check while editing text: the OS's checker behind one worker thread.
//!
//! A run's language tag (`TextStyle::locale`) picks the dictionary; untagged text
//! is checked in the system's input languages and passes when any of them knows
//! the word. A tag with no dictionary on this machine is not checked, rather than
//! flagging every word. Verdicts are cached per language and word: the canvas asks
//! for the misspelled ranges each frame and draws what is known, and a word not
//! yet checked goes to the worker, which repaints when it answers. Nothing here is
//! exported or saved; the stored text is what is checked.

#[cfg(target_os = "linux")]
mod enchant;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

/// The platform's checker. It lives on the worker thread, since the Windows one
/// is a COM object bound to the thread that made it.
pub(crate) trait Backend {
    /// The dictionary this backend checks BCP-47 `tag` with, under its own name.
    fn dictionary(&mut self, tag: &str) -> Option<String>;
    /// The system's input languages as BCP-47 tags, most preferred first.
    fn system_tags(&mut self) -> Vec<String>;
    fn check(&mut self, dict: &str, word: &str) -> bool;
    fn suggest(&mut self, dict: &str, word: &str) -> Vec<String>;
    /// Add `word` to the user's own dictionary, which the OS keeps.
    fn add(&mut self, dict: &str, word: &str);
}

/// Makes the backend, on the worker thread; `None` where the platform has none.
type Factory = Box<dyn FnOnce() -> Option<Box<dyn Backend>> + Send>;

/// How many suggestions the text menu offers.
const SUGGESTIONS: usize = 5;

/// How long the text menu waits for its suggestions before opening without them.
/// Longer under test, where a loaded machine must not turn the wait into a flake.
const SUGGEST_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(10)
} else {
    Duration::from_millis(400)
};

/// Past this many cached verdicts the cache starts over, so a long session cannot
/// grow it without bound.
const CACHE_CAP: usize = 50_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// Sent to the worker, not answered yet.
    Pending,
    Correct,
    Misspelled,
}

/// A run's language tag (`None`: the system's languages) and a word.
type Key = (Option<String>, String);

enum Job {
    Check(Option<String>, Vec<String>),
    Suggest(Option<String>, String, mpsc::Sender<Vec<String>>),
    Add(Option<String>, String),
}

/// A misspelled word the text menu was opened on.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Misspelling {
    /// Its bytes in the edited content.
    pub range: Range<usize>,
    pub tag: Option<String>,
    pub word: String,
    /// Best first; empty when the checker has none.
    pub suggestions: Vec<String>,
}

/// The app's handle on the checker.
pub(crate) struct Speller {
    /// `None` when spell check is off for this app (a test's).
    jobs: Option<mpsc::Sender<Job>>,
    verdicts: Arc<Mutex<HashMap<Key, Verdict>>>,
}

impl Speller {
    /// The platform's checker, on a worker thread that repaints `ctx` when it
    /// answers.
    pub(crate) fn system(ctx: &egui::Context) -> Self {
        let ctx = ctx.clone();
        Self::start(Box::new(platform), move || ctx.request_repaint())
    }

    /// A speller that checks nothing — what a headless app carries.
    pub(crate) fn off() -> Self {
        Self {
            jobs: None,
            verdicts: Arc::default(),
        }
    }

    fn start(factory: Factory, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let verdicts: Arc<Mutex<HashMap<Key, Verdict>>> = Arc::default();
        let shared = verdicts.clone();
        let spawned = std::thread::Builder::new()
            .name("spell".into())
            .spawn(move || work(factory(), &rx, &shared, &wake));
        Self {
            jobs: spawned.is_ok().then_some(tx),
            verdicts,
        }
    }

    /// The misspelled words of `text`, by byte range, as far as the checker has
    /// answered; unchecked words are sent to it. `runs` are `text`'s language runs
    /// (`TextEdit::locale_runs`). The word the caret touches is left alone, since
    /// it is still being typed.
    pub(crate) fn misspelled(
        &self,
        text: &str,
        runs: &[(Range<usize>, Option<String>)],
        caret: Option<usize>,
    ) -> Vec<Range<usize>> {
        let Some(jobs) = &self.jobs else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut ask: HashMap<Option<String>, Vec<String>> = HashMap::new();
        let Ok(mut verdicts) = self.verdicts.lock() else {
            return out;
        };
        if verdicts.len() > CACHE_CAP {
            verdicts.clear();
        }
        for (run, tag) in runs {
            let Some(slice) = text.get(run.clone()) else {
                continue;
            };
            for (r, word) in words(slice) {
                let r = run.start + r.start..run.start + r.end;
                if caret.is_some_and(|c| r.start <= c && c <= r.end) {
                    continue;
                }
                let key = (tag.clone(), normalized(word));
                match verdicts.get(&key) {
                    Some(Verdict::Misspelled) => out.push(r),
                    Some(_) => {}
                    None => {
                        ask.entry(key.0.clone()).or_default().push(key.1.clone());
                        verdicts.insert(key, Verdict::Pending);
                    }
                }
            }
        }
        drop(verdicts);
        for (tag, words) in ask {
            let _ = jobs.send(Job::Check(tag, words));
        }
        out
    }

    /// The misspelled word at byte `at` of `text`, with its suggestions, waiting
    /// up to [`SUGGEST_WAIT`] for them. `None` over a correct or unchecked word.
    pub(crate) fn misspelling_at(
        &self,
        text: &str,
        runs: &[(Range<usize>, Option<String>)],
        at: usize,
    ) -> Option<Misspelling> {
        let jobs = self.jobs.as_ref()?;
        // Two runs hold `at` when it is their shared boundary, and the word may
        // start the later one.
        let (r, word, tag) = runs
            .iter()
            .filter(|(r, _)| r.start <= at && at <= r.end)
            .find_map(|(run, tag)| {
                words(text.get(run.clone())?).find_map(|(r, w)| {
                    let r = run.start + r.start..run.start + r.end;
                    (r.start <= at && at <= r.end).then_some((r, w, tag))
                })
            })?;
        let key = (tag.clone(), normalized(word));
        if self.verdicts.lock().ok()?.get(&key) != Some(&Verdict::Misspelled) {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        jobs.send(Job::Suggest(key.0.clone(), key.1.clone(), tx))
            .ok()?;
        Some(Misspelling {
            range: r,
            tag: key.0,
            word: word.to_string(),
            suggestions: rx.recv_timeout(SUGGEST_WAIT).unwrap_or_default(),
        })
    }

    /// Add `word` to the user's dictionary; it stops being flagged at once.
    pub(crate) fn add(&self, tag: Option<String>, word: &str) {
        let Some(jobs) = &self.jobs else { return };
        let word = normalized(word);
        if let Ok(mut v) = self.verdicts.lock() {
            v.insert((tag.clone(), word.clone()), Verdict::Correct);
        }
        let _ = jobs.send(Job::Add(tag, word));
    }
}

/// The checkable words of `text` with their byte ranges: a word holds a letter,
/// no digit and no `.`, `_`, `@`, `/` or `:` (a number, a file name, an address),
/// is two characters or more and is not all capitals (an acronym); a script
/// without case, such as Arabic, has no capitals and is checked. Quotes at
/// either end are not part of it; an apostrophe inside is.
pub(crate) fn words(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    text.split_word_bound_indices().filter_map(|(at, w)| {
        let quote = |c: char| c == '\'' || c == '\u{2019}';
        let trimmed = w.trim_start_matches(quote);
        let start = at + (w.len() - trimmed.len());
        let word = trimmed.trim_end_matches(quote);
        let checkable = word.chars().nth(1).is_some()
            && word.chars().any(char::is_alphabetic)
            && (word.chars().any(char::is_lowercase) || !word.chars().any(char::is_uppercase))
            && !word
                .chars()
                .any(|c| c.is_numeric() || matches!(c, '.' | '_' | '@' | '/' | ':'));
        checkable.then_some((start..start + word.len(), word))
    })
}

/// A word as the checkers are asked it: a typographic apostrophe as a straight
/// one, which every dictionary spells contractions with.
fn normalized(word: &str) -> String {
    word.replace('\u{2019}', "'")
}

/// The backend where the platform has none: no dictionary, so nothing is checked.
struct Unavailable;

impl Backend for Unavailable {
    fn dictionary(&mut self, _: &str) -> Option<String> {
        None
    }
    fn system_tags(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn check(&mut self, _: &str, _: &str) -> bool {
        true
    }
    fn suggest(&mut self, _: &str, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn add(&mut self, _: &str, _: &str) {}
}

/// The worker's loop: answer each job, repainting after a check.
fn work(
    backend: Option<Box<dyn Backend>>,
    jobs: &mpsc::Receiver<Job>,
    verdicts: &Mutex<HashMap<Key, Verdict>>,
    wake: &dyn Fn(),
) {
    let mut b = backend.unwrap_or_else(|| Box::new(Unavailable));
    let mut dicts: HashMap<Option<String>, Vec<String>> = HashMap::new();
    for job in jobs {
        match job {
            Job::Check(tag, words) => {
                let ds = dictionaries(b.as_mut(), &mut dicts, &tag);
                let answers: Vec<(String, Verdict)> = words
                    .into_iter()
                    .map(|w| {
                        // No dictionary for this language: not checked.
                        let ok = ds.is_empty() || ds.iter().any(|d| b.check(d, &w));
                        let v = if ok {
                            Verdict::Correct
                        } else {
                            Verdict::Misspelled
                        };
                        (w, v)
                    })
                    .collect();
                if let Ok(mut v) = verdicts.lock() {
                    for (w, verdict) in answers {
                        v.insert((tag.clone(), w), verdict);
                    }
                }
                wake();
            }
            Job::Suggest(tag, word, reply) => {
                let mut out: Vec<String> = Vec::new();
                for d in dictionaries(b.as_mut(), &mut dicts, &tag) {
                    for s in b.suggest(&d, &word) {
                        if !out.contains(&s) {
                            out.push(s);
                        }
                    }
                }
                out.truncate(SUGGESTIONS);
                let _ = reply.send(out);
            }
            Job::Add(tag, word) => {
                if let Some(d) = dictionaries(b.as_mut(), &mut dicts, &tag).first() {
                    b.add(d, &word);
                }
            }
        }
    }
}

/// The dictionaries `tag` is checked in, resolved once per tag: its own, or for
/// untagged text one per system language, in preference order.
fn dictionaries(
    b: &mut dyn Backend,
    cache: &mut HashMap<Option<String>, Vec<String>>,
    tag: &Option<String>,
) -> Vec<String> {
    if let Some(ds) = cache.get(tag) {
        return ds.clone();
    }
    let tags = match tag {
        Some(t) => vec![t.clone()],
        None => b.system_tags(),
    };
    let mut ds: Vec<String> = Vec::new();
    for t in tags {
        if let Some(d) = b.dictionary(&t)
            && !ds.contains(&d)
        {
            ds.push(d);
        }
    }
    cache.insert(tag.clone(), ds.clone());
    ds
}

/// The platform's backend, or `None` where it cannot be made.
fn platform() -> Option<Box<dyn Backend>> {
    #[cfg(windows)]
    return windows::Windows::new().map(|b| Box::new(b) as Box<dyn Backend>);
    #[cfg(target_os = "macos")]
    return macos::MacOs::new().map(|b| Box::new(b) as Box<dyn Backend>);
    #[cfg(target_os = "linux")]
    return enchant::Enchant::new().map(|b| Box::new(b) as Box<dyn Backend>);
    #[allow(unreachable_code)]
    None
}

/// The tag a dictionary name or locale string stands for, BCP-47 style:
/// `en_US.UTF-8` → `en-US`. Shared by the backends that read POSIX names.
#[cfg_attr(windows, allow(dead_code))]
fn bcp47(name: &str) -> Option<String> {
    let name = name.split(['.', '@']).next()?.replace('_', "-");
    (!name.is_empty() && name != "C" && name != "POSIX").then_some(name)
}

/// Pick the dictionary for `tag` among `available` (BCP-47 names, compared
/// without case): the exact one, else the first of the same language — `de`
/// finds `de-DE`, `en-AU` falls back to `en-US` when there is no `en-AU`.
fn closest<'a>(tag: &str, available: &'a [String]) -> Option<&'a String> {
    let lang = |t: &str| t.split('-').next().unwrap_or(t).to_ascii_lowercase();
    available
        .iter()
        .find(|a| a.eq_ignore_ascii_case(tag))
        .or_else(|| available.iter().find(|a| lang(a) == lang(tag)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend that knows a fixed list of English words and nothing in any
    /// other language, and records what was added.
    struct Fake {
        added: Arc<Mutex<Vec<String>>>,
    }

    impl Backend for Fake {
        fn dictionary(&mut self, tag: &str) -> Option<String> {
            tag.starts_with("en").then(|| "en-US".to_string())
        }
        fn system_tags(&mut self) -> Vec<String> {
            vec!["en-GB".into(), "xx".into()]
        }
        fn check(&mut self, _: &str, word: &str) -> bool {
            ["the", "cat", "sat", "don't"].contains(&word.to_lowercase().as_str())
                || self.added.lock().unwrap().iter().any(|a| a == word)
        }
        fn suggest(&mut self, _: &str, word: &str) -> Vec<String> {
            if word == "cta" {
                vec!["cat".into(), "act".into()]
            } else {
                Vec::new()
            }
        }
        fn add(&mut self, _: &str, word: &str) {
            self.added.lock().unwrap().push(word.to_string());
        }
    }

    fn fake() -> (Speller, Arc<Mutex<Vec<String>>>) {
        let added: Arc<Mutex<Vec<String>>> = Arc::default();
        let a = added.clone();
        (
            Speller::start(
                Box::new(move || Some(Box::new(Fake { added: a }) as Box<dyn Backend>)),
                || {},
            ),
            added,
        )
    }

    /// Ask until nothing is pending: the first ask only sends the words.
    fn settle(
        s: &Speller,
        text: &str,
        runs: &[(Range<usize>, Option<String>)],
    ) -> Vec<Range<usize>> {
        for _ in 0..2000 {
            s.misspelled(text, runs, None);
            let pending = s
                .verdicts
                .lock()
                .unwrap()
                .values()
                .any(|v| *v == Verdict::Pending);
            // Asked again once settled: the answer above may predate the worker's.
            if !pending {
                return s.misspelled(text, runs, None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the worker never answered");
    }

    fn all(text: &str, tag: Option<&str>) -> Vec<(Range<usize>, Option<String>)> {
        vec![(0..text.len(), tag.map(str::to_string))]
    }

    /// Words are what a checker can judge: numbers, acronyms, addresses and lone
    /// letters are left out, and quotes are trimmed while an apostrophe stays.
    #[test]
    fn words_leave_out_what_no_dictionary_holds() {
        let text = "The 'cat' don’t NASA x 42nd a.b.c x.org sat";
        let got: Vec<&str> = words(text).map(|(_, w)| w).collect();
        assert_eq!(got, ["The", "cat", "don’t", "sat"]);
        let (r, _) = words(text).nth(1).unwrap();
        assert_eq!(&text[r], "cat", "the range is the word's, quotes outside");
    }

    /// A script without case has no capitals, so its words are not acronyms.
    ///
    /// flip: `words` asking for a lowercase letter again → fails at "Arabic".
    #[test]
    fn words_in_a_caseless_script_are_checked() {
        let got: Vec<&str> = words("مرحبا بالعالم NASA").map(|(_, w)| w).collect();
        assert_eq!(got, ["مرحبا", "بالعالم"], "Arabic");
    }

    /// The first glyph of a word that starts a language run finds the word, though
    /// the run before ends at the same byte.
    ///
    /// flip: `misspelling_at` taking only the first run holding the index → fails
    /// at "the word starting the run".
    #[test]
    fn a_misspelling_starting_a_run_is_found_at_its_first_byte() {
        let (s, _) = fake();
        let text = "the cat cta";
        let runs = vec![(0..8, None), (8..11, Some("en".to_string()))];
        settle(&s, text, &runs);
        let m = s.misspelling_at(text, &runs, 8);
        assert_eq!(m.map(|m| m.range), Some(8..11), "the word starting the run");
    }

    /// Untagged text is checked in the system languages; the misspelled word comes
    /// back by range once the worker has answered, and not before.
    #[test]
    fn a_misspelled_word_is_flagged_once_checked() {
        let (s, _) = fake();
        let text = "the cta sat";
        assert!(
            s.misspelled(text, &all(text, None), None).is_empty(),
            "nothing is known on the first ask"
        );
        assert_eq!(settle(&s, text, &all(text, None)), vec![4..7]);
    }

    /// A run tagged with a language this machine has no dictionary for is left
    /// unchecked rather than flagged word by word.
    #[test]
    fn a_language_without_a_dictionary_is_not_checked() {
        let (s, _) = fake();
        let text = "der Hund";
        assert!(settle(&s, text, &all(text, Some("de"))).is_empty());
        let text = "the cta";
        assert_eq!(settle(&s, text, &all(text, Some("en-AU"))), vec![4..7]);
    }

    /// The word under the caret is still being typed and is not flagged.
    #[test]
    fn the_word_at_the_caret_is_left_alone() {
        let (s, _) = fake();
        let text = "the cta";
        settle(&s, text, &all(text, None));
        assert!(s.misspelled(text, &all(text, None), Some(7)).is_empty());
        assert!(s.misspelled(text, &all(text, None), Some(5)).is_empty());
        assert_eq!(s.misspelled(text, &all(text, None), Some(3)), vec![4..7]);
    }

    /// The menu's question: the misspelled word at an index, with suggestions;
    /// adding it to the dictionary clears it at once and reaches the backend.
    #[test]
    fn a_misspelling_offers_suggestions_and_can_be_added() {
        let (s, added) = fake();
        let text = "the cta";
        let runs = all(text, None);
        settle(&s, text, &runs);
        assert_eq!(s.misspelling_at(text, &runs, 1), None, "a correct word");
        let m = s
            .misspelling_at(text, &runs, 5)
            .expect("the misspelled word");
        assert_eq!(m.range, 4..7);
        assert_eq!(m.suggestions, ["cat", "act"]);
        s.add(m.tag, &m.word);
        assert!(s.misspelled(text, &runs, None).is_empty());
        for _ in 0..2000 {
            if !added.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*added.lock().unwrap(), ["cta"]);
    }

    /// The whole path in the app: a text session's misspelled word is found under a
    /// point, its menu leads with the suggestions, and taking one rewrites the word
    /// in the editor; with the setting off the menu has no spelling rows.
    /// flip: `misspelling_at` mapping the point without the layer's transform →
    /// fails at "the word under the pointer", the text sitting away from the origin.
    #[test]
    fn a_suggestion_from_the_text_menu_replaces_the_word() {
        use crate::app::OndinApp;
        use crate::menu::{Item, Target};
        use ondin_core::kurbo::{Affine, Point};
        use ondin_core::{NodeKind, Operation, TextParts, TextSizing, TextStyle, Transaction};

        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.spell = fake().0;
        let page = app.session.doc.first_page();
        let id = app.session.ids.mint();
        // The misspelling first: a point that misses the text clamps to its end,
        // which must be a correct word for the flip to bite.
        let content = "cta sat";
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Text {
                content: content.into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: 20.0,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((300.0, 200.0))),
            name: None,
        }])));
        let node = app.session.doc.get(id).expect("the text");
        let parts = TextParts::of(node.kind()).expect("a text kind");
        let editor = ondin_core::TextEdit::new(content, parts);
        let runs = editor.locale_runs();
        assert_eq!(settle(&app.spell, content, &runs), vec![0..3]);
        let quad = editor.range_quads(0..3)[0];
        let local = Point::new((quad[0].x + quad[2].x) / 2.0, (quad[0].y + quad[2].y) / 2.0);
        app.text = Some(crate::preview::TextSession {
            id,
            editor,
            original: (content.into(), Default::default(), Default::default()),
            created: false,
            blink: Default::default(),
        });
        app.mode = crate::input::Mode::TextInsert;
        let world = app.session.preview_world_transform(id).expect("placed") * local;

        let m = app
            .misspelling_at(world)
            .expect("the word under the pointer");
        assert_eq!(
            (m.range.clone(), m.suggestions.clone()),
            (0..3, vec!["cat".into(), "act".into()])
        );
        let menu = app.menu_snapshot(egui::Pos2::ZERO, Target::TextSession, Some(world));
        let kinds = [];
        let rows: Vec<String> = crate::menu::build(&app.menu_context(&menu, &kinds))
            .into_iter()
            .flatten()
            .map(|r| r.label.into_owned())
            .collect();
        assert_eq!(
            rows[..3],
            ["cat", "act", "Add to dictionary"],
            "the corrections lead the menu"
        );

        app.spelling_row(Item::Suggestion(0), m);
        assert_eq!(app.text.as_ref().unwrap().editor.content(), "cat sat");

        app.prefs.spell_check = false;
        let menu = app.menu_snapshot(egui::Pos2::ZERO, Target::TextSession, Some(world));
        assert_eq!(menu.spelling, None, "off, the menu asks nothing");
    }

    /// A speller that is off checks nothing and offers nothing.
    #[test]
    fn an_off_speller_is_silent() {
        let s = Speller::off();
        let text = "the cta";
        assert!(s.misspelled(text, &all(text, None), None).is_empty());
        assert_eq!(s.misspelling_at(text, &all(text, None), 5), None);
    }

    /// Dictionary names come back as tags, and a tag finds its language's.
    #[test]
    fn tags_and_dictionaries_meet() {
        assert_eq!(bcp47("en_US.UTF-8").as_deref(), Some("en-US"));
        assert_eq!(bcp47("C"), None);
        let available = ["en-US".to_string(), "de-DE".to_string()];
        assert_eq!(closest("EN-us", &available), Some(&available[0]));
        assert_eq!(closest("de", &available), Some(&available[1]));
        assert_eq!(closest("en-AU", &available), Some(&available[0]));
        assert_eq!(closest("fr", &available), None);
    }
}
