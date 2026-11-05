//! What each pinned version changed, as *Version history* says it under the
//! version's name: the diff (`ondin_core::diff`) of the version against the one
//! pinned before it, folded as the compare folds it (`compare::topmost`): a count is
//! of topmost changed layers, a page's own change counts once, and a page that came
//! or went counts the layers inside it.
//!
//! **Worked out once, off the UI thread.** A pinned version never changes, so a
//! pair's summary is cached by the two paths for as long as the app runs and the
//! card still lists the pair, its lines cut to what an opened row shows. The open
//! document's line, against the newest version, is cached by its revision. One
//! pair is worked on at a time, the card's order deciding which ([`Diffs::want`]).

use crate::job::Job;
use ondin_core::diff::{self, Change, Status};
use ondin_core::{Document, NodeId};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A row's line names this many pages, then says how many more.
const PAGES_NAMED: usize = 2;
/// An opened row lists this many layers a page, then says how many more; a
/// summary keeps no more lines than that, since the cache outlives the card.
pub(crate) const LINES_SHOWN: usize = 4;

/// The later side of a pair: a pinned version, or the open document at a revision.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Later {
    Version(PathBuf),
    Now(u64),
}

/// Two states whose diff a row shows: the earlier version (`None` for the oldest,
/// which is compared with nothing) and the later.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Pair {
    pub(crate) older: Option<PathBuf>,
    pub(crate) newer: Later,
}

/// How many topmost layers changed, came and went.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) changed: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

impl Counts {
    fn add(&mut self, status: Status, n: usize) {
        match status {
            Status::Changed => self.changed += n,
            Status::Added => self.added += n,
            Status::Removed => self.removed += n,
        }
    }

    fn plus(self, o: Counts) -> Counts {
        Counts {
            changed: self.changed + o.changed,
            added: self.added + o.added,
            removed: self.removed + o.removed,
        }
    }

    /// Each kind that happened, in the order `~ + −`.
    pub(crate) fn each(self) -> impl Iterator<Item = (Status, usize)> {
        [
            (Status::Changed, self.changed),
            (Status::Added, self.added),
            (Status::Removed, self.removed),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
    }
}

/// One topmost changed layer, or a page's own change, as an opened row lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) status: Status,
    /// The names above it, each followed by ` / `.
    pub(crate) trail: String,
    pub(crate) name: String,
    /// Its name as private mode draws it.
    pub(crate) masked: String,
    /// What changed, named without values: `fill, height`, or `6 layers`.
    pub(crate) what: String,
}

/// One page the pair differs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PageBrief {
    pub(crate) name: String,
    pub(crate) masked: String,
    pub(crate) status: Status,
    pub(crate) counts: Counts,
    /// The first [`LINES_SHOWN`] lines.
    pub(crate) lines: Vec<Line>,
    /// How many more there were.
    pub(crate) more: usize,
}

/// Something outside every page that differs: a variable, a style, a guide…
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) status: Status,
    /// Its noun: `variable`, `style`, `library`.
    pub(crate) kind: String,
    /// Its noun and its name or place: `variable “brand/500”`.
    pub(crate) subject: String,
}

/// What a pair's diff says, in brief.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Brief {
    /// In the later document's order, removed pages last.
    pub(crate) pages: Vec<PageBrief>,
    pub(crate) document: Vec<Item>,
}

/// A row's summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Summary {
    /// The two are the same.
    Same,
    /// Nothing to compare with: the document's size.
    Size {
        pages: usize,
        layers: usize,
    },
    Changed(Brief),
    /// A side could not be read: the row's own version when `this`, else the one
    /// before it (or the work died).
    Failed {
        this: bool,
    },
}

/// One run of a row's summary line: words in the dim ink, or a count in its
/// status's hue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Words(String),
    Count(Status, usize),
}

impl Brief {
    pub(crate) fn counts(&self) -> Counts {
        self.pages
            .iter()
            .fold(Counts::default(), |sum, p| sum.plus(p.counts))
    }

    /// The summary line: the pages it touched — one by name, a page that came or
    /// went said so, several two at most and how many more — then the counts, then
    /// what changed off the pages by kind.
    pub(crate) fn parts(&self, private: bool) -> Vec<Part> {
        let name = |p: &PageBrief| match private {
            true => p.masked.clone(),
            false => p.name.clone(),
        };
        let mut out = Vec::new();
        match self.pages.as_slice() {
            [] => {}
            [p] => out.push(Part::Words(match p.status {
                Status::Added => format!("New page {}", name(p)),
                Status::Removed => format!("Removed page {}", name(p)),
                Status::Changed => name(p),
            })),
            pages => {
                let named: Vec<String> = pages.iter().take(PAGES_NAMED).map(name).collect();
                let mut words = named.join(", ");
                if pages.len() > PAGES_NAMED {
                    words.push_str(&format!(" and {} more", pages.len() - PAGES_NAMED));
                }
                out.push(Part::Words(words));
            }
        }
        out.extend(self.counts().each().map(|(s, n)| Part::Count(s, n)));
        let kinds = self.kinds();
        if !kinds.is_empty() {
            let words = kinds.join(", ");
            out.push(Part::Words(match self.pages.is_empty() {
                true => words,
                false => format!("· {words}"),
            }));
        }
        out
    }

    /// What changed off the pages, by kind in the order first met: `2 variables`,
    /// and a block that is not a list by its noun alone (`library`).
    fn kinds(&self) -> Vec<String> {
        let mut kinds: Vec<(&str, usize)> = Vec::new();
        for item in &self.document {
            match kinds.iter_mut().find(|(k, _)| *k == item.kind) {
                Some((_, n)) => *n += 1,
                None => kinds.push((&item.kind, 1)),
            }
        }
        kinds
            .into_iter()
            .map(|(kind, n)| match kind {
                "variable" | "mode" | "style" | "guide" | "image" => crate::ui::count(n, kind),
                "kit" => "kit publish".into(),
                other => other.to_string(),
            })
            .collect()
    }
}

/// The layers on `doc`'s pages, the pages themselves not counted.
pub(crate) fn layer_count(doc: &Document) -> usize {
    let mut n = 0;
    let mut stack: Vec<NodeId> = doc.page_ids().collect();
    while let Some(at) = stack.pop() {
        let Some(node) = doc.get(at) else { continue };
        n += usize::from(!doc.is_page(at));
        stack.extend(node.children());
    }
    n
}

/// The summary of `newer` against `older`, or of `newer` alone when it is the
/// oldest. Names are masked here as well as kept, since private mode may come on
/// while the summary is cached.
pub(crate) fn summarize(older: Option<&Document>, newer: &Document) -> Summary {
    let Some(older) = older else {
        return Summary::Size {
            pages: newer.pages().len(),
            layers: layer_count(newer),
        };
    };
    let d = diff::diff(older, newer);
    if d.is_empty() {
        return Summary::Same;
    }
    let (then, now) = (older, newer);
    // `private` masks by a thread-local switch: on, it masks on this thread alone.
    let was = crate::private::on();
    crate::private::set(true);
    let mut pages = Vec::new();
    for page in &d.pages {
        let id = NodeId::from_wire(&page.id);
        let holder = |id: NodeId| if now.contains(id) { now } else { then };
        let masked = id.map_or_else(
            || "Page".into(),
            |id| crate::private::page(holder(id), id).into_owned(),
        );
        let mut brief = PageBrief {
            name: page.name.clone(),
            masked,
            status: page.status,
            counts: Counts::default(),
            lines: Vec::new(),
            more: 0,
        };
        if page.status != Status::Changed {
            brief.counts.add(page.status, page.inside);
            pages.push(brief);
            continue;
        }
        if !page.changes.is_empty() {
            brief.counts.changed += 1;
            brief.lines.push(Line {
                status: Status::Changed,
                trail: String::new(),
                name: page.name.clone(),
                masked: brief.masked.clone(),
                what: whats(&page.changes),
            });
        }
        let tops = crate::compare::topmost(&page.layers, then, now);
        // Each topmost layer's line, and how many listed layers folded into it.
        let mut line_of: HashMap<usize, (usize, usize)> = HashMap::new();
        for (i, layer) in page.layers.iter().enumerate() {
            let (Some(id), Some(top)) = (NodeId::from_wire(&layer.id), tops[i]) else {
                continue;
            };
            if top != i {
                if let Some((_, folded)) = line_of.get_mut(&top) {
                    *folded += 1;
                }
                continue;
            }
            brief.counts.add(layer.status, 1);
            let what = match layer.status {
                Status::Changed => whats(&layer.changes),
                _ if layer.inside > 0 => crate::ui::count(layer.inside + 1, "layer"),
                _ => String::new(),
            };
            let above = &layer.trail[..layer.trail.len().saturating_sub(1)];
            line_of.insert(i, (brief.lines.len(), 0));
            brief.lines.push(Line {
                status: layer.status,
                trail: above.iter().map(|n| format!("{n} / ")).collect(),
                name: layer.trail.last().cloned().unwrap_or_default(),
                masked: crate::private::layer(holder(id), id).into_owned(),
                what,
            });
        }
        // A changed layer whose children changed too says how many.
        for (line, folded) in line_of.into_values() {
            let l = &mut brief.lines[line];
            if folded > 0 && l.status == Status::Changed {
                let inside = crate::ui::count(folded, "layer") + " inside";
                l.what = match l.what.is_empty() {
                    true => inside,
                    false => format!("{}, {inside}", l.what),
                };
            }
        }
        brief.more = brief.lines.len().saturating_sub(LINES_SHOWN);
        brief.lines.truncate(LINES_SHOWN);
        pages.push(brief);
    }
    crate::private::set(was);
    let mut document: Vec<Item> = Vec::new();
    for c in &d.document {
        match document.iter_mut().find(|i| i.subject == c.subject()) {
            Some(item) => item.status = Status::Changed,
            None => document.push(Item {
                status: status_of(c),
                kind: c.kind().to_string(),
                subject: c.subject().to_string(),
            }),
        }
    }
    Summary::Changed(Brief { pages, document })
}

/// What `changes` touched, each named once by its first word, in order.
fn whats(changes: &[Change]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for c in changes {
        let word = c.what.split(' ').next().unwrap_or_default();
        if !out.contains(&word) {
            out.push(word);
        }
    }
    out.join(", ")
}

/// A change outside every page: added with no value before, removed with none after.
fn status_of(c: &Change) -> Status {
    match (&c.before, &c.after) {
        (None, Some(_)) => Status::Added,
        (Some(_), None) => Status::Removed,
        _ => Status::Changed,
    }
}

fn load(path: &Path) -> Result<Document, ()> {
    let bytes = std::fs::read(path).map_err(drop)?;
    ondin_core::io::load(&bytes).map_err(drop)
}

/// Work out `pair`'s summary; `now` is the open document when the later side is it.
fn work(pair: &Pair, now: Option<Document>) -> Summary {
    let newer = match (&pair.newer, now) {
        (_, Some(doc)) => doc,
        (Later::Version(path), None) => match load(path) {
            Ok(doc) => doc,
            Err(()) => return Summary::Failed { this: true },
        },
        (Later::Now(_), None) => return Summary::Failed { this: false },
    };
    let older = match pair.older.as_deref().map(load).transpose() {
        Ok(doc) => doc,
        Err(()) => return Summary::Failed { this: false },
    };
    summarize(older.as_ref(), &newer)
}

/// A pair waiting its turn, with the open document's snapshot when it is one side.
struct Task {
    pair: Pair,
    now: Option<Document>,
}

/// The summaries worked out, and the queue of those still wanted.
#[derive(Default)]
pub(crate) struct Diffs {
    done: HashMap<Pair, Arc<Summary>>,
    /// Front first.
    queue: Vec<Task>,
    running: Option<(Pair, Job<Summary>)>,
    /// When each pair still pending was first asked for, in egui's seconds.
    asked: HashMap<Pair, f64>,
}

impl Diffs {
    /// `pair`'s summary, once worked out.
    pub(crate) fn get(&self, pair: &Pair) -> Option<Arc<Summary>> {
        self.done.get(pair).cloned()
    }

    /// When `pair` was first asked for and is still being worked out.
    pub(crate) fn asked(&self, pair: &Pair) -> Option<f64> {
        self.asked.get(pair).copied()
    }

    /// Ask for `pairs`, in the order they should be worked: those not done join the
    /// queue's back in that order, and `first`, when given, goes to its front.
    /// `now` makes the open document's snapshot for a pair whose later side is it,
    /// once, when it is queued.
    pub(crate) fn want(
        &mut self,
        pairs: &[Pair],
        first: Option<&Pair>,
        at: f64,
        now: impl Fn() -> Document,
    ) {
        let mut fresh = false;
        for pair in pairs {
            let running = self.running.as_ref().is_some_and(|(p, _)| p == pair);
            if self.done.contains_key(pair) || running {
                continue;
            }
            self.asked.entry(pair.clone()).or_insert(at);
            if !self.queue.iter().any(|t| &t.pair == pair) {
                fresh = true;
                // The open document's line for an older revision is never shown again.
                if matches!(pair.newer, Later::Now(_)) {
                    self.done.retain(|p, _| !matches!(p.newer, Later::Now(_)));
                    self.queue
                        .retain(|t| !matches!(t.pair.newer, Later::Now(_)));
                }
                let now = matches!(pair.newer, Later::Now(_)).then(&now);
                self.queue.push(Task {
                    pair: pair.clone(),
                    now,
                });
            }
        }
        // A new pair is a new pin, which may have pruned the oldest: the pairs of
        // these versions' folder that are no longer asked for go.
        if fresh {
            let folder = |p: &Pair| match &p.newer {
                Later::Version(v) => v.parent().map(Path::to_path_buf),
                Later::Now(_) => None,
            };
            let folders: HashSet<PathBuf> = pairs.iter().filter_map(folder).collect();
            let wanted: HashSet<&Pair> = pairs.iter().collect();
            self.done.retain(|p, _| {
                wanted.contains(p) || folder(p).is_none_or(|f| !folders.contains(&f))
            });
        }
        if let Some(first) = first
            && let Some(i) = self.queue.iter().position(|t| &t.pair == first)
        {
            let task = self.queue.remove(i);
            self.queue.insert(0, task);
        }
    }

    /// Land a finished summary and start the next; answers whether one landed.
    pub(crate) fn pump(&mut self, ctx: &egui::Context) -> bool {
        let mut landed = false;
        if let Some((pair, job)) = &self.running {
            let answer = match job.poll() {
                Ok(None) => return false,
                Ok(Some(summary)) => summary,
                Err(()) => Summary::Failed { this: false },
            };
            self.asked.remove(pair);
            self.done.insert(pair.clone(), Arc::new(answer));
            self.running = None;
            landed = true;
        }
        if !self.queue.is_empty() {
            let Task { pair, now } = self.queue.remove(0);
            let key = pair.clone();
            let job = Job::spawn("version-diff", ctx, move || work(&key, now));
            self.running = Some((pair, job));
        }
        landed
    }

    /// Stop: the queue is dropped; what finished stays, and the pair in hand is
    /// kept to land at the next [`Self::pump`], since its thread cannot be stopped
    /// and a reopened card asking for it again would run a second beside it.
    pub(crate) fn stop(&mut self) {
        self.queue.clear();
        let running = self.running.as_ref().map(|(p, _)| p);
        self.asked.retain(|p, _| Some(p) == running);
    }

    /// Work out everything queued on this thread (tests only).
    #[cfg(test)]
    pub(crate) fn settle(&mut self) {
        if let Some((pair, job)) = self.running.take() {
            let summary = job.wait().unwrap_or(Summary::Failed { this: false });
            self.asked.remove(&pair);
            self.done.insert(pair, Arc::new(summary));
        }
        for Task { pair, now } in std::mem::take(&mut self.queue) {
            let summary = work(&pair, now);
            self.asked.remove(&pair);
            self.done.insert(pair, Arc::new(summary));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(name: &str, status: Status, counts: Counts) -> PageBrief {
        PageBrief {
            name: name.into(),
            masked: "Page 9".into(),
            status,
            counts,
            lines: Vec::new(),
            more: 0,
        }
    }

    fn item(kind: &str, subject: &str) -> Item {
        Item {
            status: Status::Changed,
            kind: kind.into(),
            subject: subject.into(),
        }
    }

    fn words(w: &str) -> Part {
        Part::Words(w.into())
    }

    /// `summarize` counts topmost changed layers as the compare does: a changed
    /// layer under a changed frame is the frame's *1 layer inside*, a new frame
    /// counts once, and a layer moved into it counts on its own.
    ///
    /// flip: `summarize` counting every listed layer (no `top != i` arm) → fails
    /// at the counts, the price counted; `topmost` folding into an added row →
    /// fails at the counts, the badge lost.
    #[test]
    fn a_summary_counts_the_topmost_changed_layers() {
        use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
        use ondin_core::{NodeKind, Operation, Transaction};
        let mut ids = ondin_core::IdSource::new(0xE1);
        let root = ids.mint();
        let [card, price, badge, old, tray] = [(); 5].map(|_| ids.mint());
        let boxed = |id, parent, name: &str| Operation::CreateNode {
            id,
            parent,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some(name.into()),
        };
        let mut then = Document::new(root);
        then.apply(&Transaction(vec![
            boxed(card, root, "Card"),
            boxed(price, card, "Price"),
            boxed(badge, root, "Badge"),
            boxed(old, root, "Old"),
        ]))
        .unwrap();
        let mut now = then.clone();
        let fade = |id| Operation::SetOpacity { id, opacity: 0.5 };
        now.apply(&Transaction(vec![
            Operation::SetName {
                id: card,
                name: "Studio".into(),
            },
            fade(price),
            boxed(tray, root, "Tray"),
            Operation::Reparent {
                id: badge,
                new_parent: tray,
                index: 0,
            },
            fade(badge),
            Operation::DeleteNode { id: old },
        ]))
        .unwrap();
        let Summary::Changed(brief) = summarize(Some(&then), &now) else {
            panic!("a change");
        };
        let page = &brief.pages[0];
        let n = |changed, added, removed| Counts {
            changed,
            added,
            removed,
        };
        assert_eq!(page.counts, n(2, 1, 1), "{:#?}", page.lines);
        let line = |name: &str| page.lines.iter().find(|l| l.name == name);
        assert_eq!(
            line("Studio").map(|l| l.what.as_str()),
            Some("name, 1 layer inside")
        );
        assert!(line("Badge").is_some_and(|l| l.status == Status::Changed));
        assert!(line("Price").is_none(), "folded into the card");
    }

    /// A stop keeps the pair in hand, so a reopened card asking for it again runs
    /// no second; a new pin's pair drops the pairs of its folder no longer listed.
    ///
    /// flip: `Diffs::stop` dropping `running` → fails at "not run again".
    #[test]
    fn a_stop_keeps_the_pair_in_hand_and_a_pin_drops_a_pruned_pair() {
        let ctx = egui::Context::default();
        let dir = std::env::temp_dir().join("ondin-diffs-missing");
        let pair = |newer: &str, older: Option<&str>| Pair {
            older: older.map(|o| dir.join(o)),
            newer: Later::Version(dir.join(newer)),
        };
        let none = || -> Document { unreachable!("no pair is the open document's") };
        let mut d = Diffs::default();
        let (one, two) = (pair("1.ondin", None), pair("2.ondin", Some("1.ondin")));
        d.want(&[two.clone(), one.clone()], None, 0.0, none);
        d.pump(&ctx);
        d.stop();
        d.want(&[two.clone(), one.clone()], None, 1.0, none);
        assert_eq!(d.queue.len(), 1, "the pair in hand is not run again");
        d.settle();
        assert!(d.get(&two).is_some() && d.get(&one).is_some());
        // `3` pinned, `1` pruned: `2` is now the oldest.
        let (three, oldest) = (pair("3.ondin", Some("2.ondin")), pair("2.ondin", None));
        d.want(&[three, oldest], None, 2.0, none);
        assert!(d.get(&one).is_none() && d.get(&two).is_none(), "pruned");
    }

    /// The summary line names two pages and how many more, totals the counts in the
    /// order `~ + −` leaving out a zero, and counts what changed off the pages by
    /// kind; one page that came reads as a new page, by its masked name in private.
    ///
    /// flip: `Counts::each` keeping a count of 0 → fails at the second `assert_eq`.
    #[test]
    fn the_summary_line_names_pages_counts_and_kinds() {
        let n = |changed, added, removed| Counts {
            changed,
            added,
            removed,
        };
        let brief = Brief {
            pages: vec![
                page("Tables", Status::Changed, n(9, 4, 0)),
                page("Schema", Status::Changed, n(3, 0, 0)),
                page("Drafts", Status::Removed, n(0, 0, 5)),
            ],
            document: vec![
                item("variable", "variable “a”"),
                item("variable", "variable “b”"),
                item("library", "library"),
            ],
        };
        assert_eq!(
            brief.parts(false),
            [
                words("Tables, Schema and 1 more"),
                Part::Count(Status::Changed, 12),
                Part::Count(Status::Added, 4),
                Part::Count(Status::Removed, 5),
                words("· 2 variables, library"),
            ]
        );
        let added = Brief {
            pages: vec![page("Schema", Status::Added, n(0, 18, 0))],
            document: Vec::new(),
        };
        assert_eq!(
            added.parts(false),
            [words("New page Schema"), Part::Count(Status::Added, 18)]
        );
        assert_eq!(added.parts(true)[0], words("New page Page 9"));
        let off = Brief {
            pages: Vec::new(),
            document: vec![item("style", "style “H1”")],
        };
        assert_eq!(off.parts(false), [words("1 style")]);
    }
}
