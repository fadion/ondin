//! What the readers of other tools' files share (`sketch_in`, `penpot_in`):
//! where entries come from, how what did not map is counted, and the handful of
//! JSON readings both formats need.

use crate::image::ImageFormat;
use crate::node::Fill;
use crate::peniko::{self, Color};
use crate::typography::{CharAttrKind, TextStyle};
use serde_json::Value;
use std::collections::BTreeMap;

/// Where a file's entries are read from: the zip, in the app.
pub trait Archive {
    /// The bytes of the entry at `name`, or `None` when there is no such entry.
    fn entry(&mut self, name: &str) -> Option<Vec<u8>>;

    /// Every entry's name, for a format whose index does not list its own
    /// folders. Empty unless the archive can say.
    fn names(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// One kind of thing that did not come in as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unmapped {
    pub what: &'static str,
    /// How many, with its noun where a bare number would not say of what.
    pub count: String,
    pub how: &'static str,
    /// What the row is about, so an import that leaves that part out can drop it.
    pub part: Part,
}

/// The part of a document a report row is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Layers,
    Components,
    Pictures,
}

impl Part {
    /// The part each row is about, by its words.
    fn of(what: &str) -> Self {
        let lower = what.to_ascii_lowercase();
        if lower.starts_with("picture") {
            Self::Pictures
        } else if lower.contains("symbol")
            || lower.contains("component")
            || lower.contains("instance")
            || lower.contains("variant")
        {
            Self::Components
        } else {
            Self::Layers
        }
    }
}

/// What did not map, counted by kind as a reader meets it.
#[derive(Default)]
pub(crate) struct Tally(BTreeMap<&'static str, (usize, &'static str, &'static str)>);

impl Tally {
    /// Count `n` more of `what`, in `unit` (a plural, or empty), with `how` it
    /// came in.
    pub(crate) fn count(
        &mut self,
        what: &'static str,
        n: usize,
        unit: &'static str,
        how: &'static str,
    ) {
        self.0.entry(what).or_insert((0, unit, how)).0 += n;
    }

    /// One row per kind, in a stable order.
    pub(crate) fn into_report(self) -> Vec<Unmapped> {
        self.0
            .into_iter()
            .map(|(what, (n, unit, how))| Unmapped {
                what,
                count: match (unit, n) {
                    ("", _) => n.to_string(),
                    // Every unit is a plural in `s`; one of it drops the `s`.
                    (unit, 1) => format!("1 {}", unit.strip_suffix('s').unwrap_or(unit)),
                    (unit, n) => format!("{n} {unit}"),
                },
                how,
                part: Part::of(what),
            })
            .collect()
    }
}

/// The names given out so far, per kind of name. Other tools let two styles or
/// two colours share a name, which a document refuses, so a repeat is numbered
/// apart: the second "Shadow" becomes "Shadow 2".
pub(crate) struct Names<K>(rustc_hash::FxHashSet<(K, String)>);

impl<K> Default for Names<K> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<K: Copy + Eq + std::hash::Hash> Names<K> {
    /// `name`, or the first free numbering of it, among `kind`'s; `blank` stands
    /// in for an empty one. Counts each renaming on `tally`.
    pub(crate) fn unique(&mut self, kind: K, name: &str, blank: &str, tally: &mut Tally) -> String {
        let stem = if name.trim().is_empty() { blank } else { name };
        let mut out = stem.to_owned();
        let mut n = 1;
        while !self.0.insert((kind, out.clone())) {
            n += 1;
            out = format!("{stem} {n}");
        }
        if n > 1 {
            tally.count("Names used twice", 1, "", "Numbered apart");
        }
        out
    }
}

/// A picture's format by its first bytes.
pub(crate) fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF8") {
        Some(ImageFormat::Gif)
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

/// A map from a UTF-16 offset, which Sketch's runs and Figma's overrides count
/// in, to a byte offset into `s`.
pub(crate) fn utf16_to_bytes(s: &str) -> impl Fn(usize) -> usize + use<> {
    let mut table = Vec::with_capacity(s.len() + 1);
    for (byte, c) in s.char_indices() {
        for _ in 0..c.len_utf16() {
            table.push(byte);
        }
    }
    table.push(s.len());
    move |u| table.get(u).copied().unwrap_or(*table.last().unwrap_or(&0))
}

/// A visible fill of `c`.
pub(crate) fn solid(c: Color) -> Fill {
    Fill {
        brush: peniko::Brush::Solid(c).into(),
        visible: true,
    }
}

/// `style` with every attribute put through [`TextStyle::set`], so a default
/// read from a file holds the canonical lengths and the size range a span over
/// it holds, and equals that span where the file meant the same value.
pub(crate) fn canonical_style(style: TextStyle) -> TextStyle {
    let mut out = style.clone();
    for kind in CharAttrKind::ALL {
        out.set(style.get(kind));
    }
    out
}

pub(crate) fn num(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

pub(crate) fn b_of(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

pub(crate) fn arr<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v.get(k)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}
