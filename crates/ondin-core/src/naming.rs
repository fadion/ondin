//! Layer names the app generates, and how they get their numbers (§15 D127).
//!
//! Every create site passes `name: None` and `Document::op_create` turns that
//! into a name. Uniqueness is never load-bearing: nothing resolves a node by
//! name (lookups are by `NodeId`), so the numbering is a courtesy and every
//! collision it declines to prevent is legal.
//!
//! Creating numbers within the parent from the kind's stem, starting at 1
//! ([`free_name`]); copying keeps the name unless it looks numbered, in which
//! case it increments ([`copy_name`]). Neither runs again afterwards:
//! reparenting may collide, because renaming on move would change names behind
//! the user's back.
//!
//! There is no stored "auto name" flag and no persisted counter; both are read
//! back out of the names present, which makes freed numbers reusable and keeps
//! numbering from resetting on reload.

use crate::node::{BoolOp, NodeKind};
use rustc_hash::FxHashSet;

/// Every name [`NodeKind::default_name`] can produce.
///
/// A list, because "would we have written this string" is the inverse question a
/// `match` on a kind cannot answer from a name. The test
/// `every_default_name_is_listed_as_generated` keeps it in step with `default_name`.
pub const GENERATED_NAMES: &[&str] = &[
    "Root",
    "Frame",
    "Group",
    "Rectangle",
    "Ellipse",
    "Polygon",
    "Star",
    "Line",
    "Path",
    "Text",
    "Union",
    "Subtract",
    "Intersect",
    "Exclude",
];

/// Whether `name` is a **stem** this app numbers from, as opposed to a word a
/// person typed.
///
/// Asked about the part in front of a number, and about a bare name only where an
/// older document might carry one. Exact: "Rectangle" is a stem, "Rectangles" is not.
pub fn is_generated(name: &str) -> bool {
    GENERATED_NAMES.contains(&name)
}

/// A caller-supplied name reduced to one this app can show, or `None` when there
/// is nothing usable in it (§15 D644).
///
/// Trimmed; every interior whitespace character becomes one ordinary space, since
/// the layers panel is one line. Runs are not collapsed (`"Hello   World"` stays),
/// the way [`numbered`] declines to read `"Layer 007"` as a number. `None` rather
/// than a repaired string lets `op_create` fall back to a generated name, as for
/// an element with no `id`. This is about whether a name is displayable, not
/// unique.
#[must_use]
pub fn usable(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() {
        return None;
    }
    Some(
        name.chars()
            .map(|c| if c.is_whitespace() { ' ' } else { c })
            .collect(),
    )
}

/// The stem and number of a numbered name — `"Rectangle 3"` → `("Rectangle", 3)`.
///
/// The tail must be the canonical decimal it parses back to, which rejects
/// `"Layer 007"`, `"Layer +3"` and `"Layer 3 "`: a name we did not write is the
/// user's. Zero is rejected too, since the sequence starts at 1.
pub fn numbered(name: &str) -> Option<(&str, u32)> {
    let (stem, tail) = name.rsplit_once(' ')?;
    if stem.is_empty() {
        return None;
    }
    let n: u32 = tail.parse().ok()?;
    (n >= 1 && n.to_string() == tail).then_some((stem, n))
}

/// The generated stem behind `name`, if the app is the one that wrote it —
/// `"Union"` and `"Union 2"` both give `"Union"`, and `"Badge 2"` gives nothing.
///
/// Narrower than [`copy_name`]'s test on purpose: copying increments anything that
/// looks numbered (a benign false positive), while this asks whether we would have
/// generated the string, so `build::set_boolean_op` never renames a user's layer.
/// The two must not be unified.
pub fn generated_stem(name: &str) -> Option<&str> {
    let stem = numbered(name).map_or(name, |(stem, _)| stem);
    is_generated(stem).then_some(stem)
}

/// Whether `name` is the app's own name for a boolean of some operation —
/// `build::set_boolean_op`'s "may I rename this" test.
///
/// Uses the strict [`generated_stem`] test.
pub fn is_boolean_label(name: &str) -> bool {
    generated_stem(name).is_some_and(|stem| BoolOp::ALL.iter().any(|op| op.label() == stem))
}

/// The first name in the sequence `stem <from>`, `stem <from+1>`, … that `taken`
/// does not already hold.
///
/// Every generated name carries a number, the first included, and `from` is at
/// least 1. It is the first free number, not one past the highest, so deleting
/// "Rectangle 2" frees it with no stored state. `from` is what separates copying
/// from creating: a copy of "Rectangle 3" starts at 4.
///
/// The search wraps to 1 at `u32::MAX` and that is load-bearing (§15 D433):
/// `copy_name`'s `from` is parsed from a user-typed string, so duplicating
/// `Rectangle 4294967295` would overflow (debug panic) or yield `Rectangle 0`,
/// which `numbered` refuses. The final line is unreachable in memory; it answers a
/// legal duplicate rather than an unparseable name.
pub fn free_name(taken: &FxHashSet<&str>, stem: &str, from: u32) -> String {
    for start in [from.max(1), 1] {
        for n in start..=u32::MAX {
            let candidate = format!("{stem} {n}");
            if !taken.contains(candidate.as_str()) {
                return candidate;
            }
        }
    }
    format!("{stem} {}", from.max(1))
}

/// The name a copy of a layer called `current` should carry among siblings called
/// `taken`, or `None` to keep the name it has.
///
/// A duplicate keeps its name unless it is numbered: "Badge" stays "Badge", but
/// "Rectangle 2" becomes "Rectangle 3". Pattern-matched rather than flagged, since
/// an "is auto" bit would have to live in the model and the false positive is
/// benign. A bare generated name ("Rectangle", from an older document or typed)
/// joins the series from 1.
pub fn copy_name(taken: &FxHashSet<&str>, current: &str) -> Option<String> {
    match numbered(current) {
        Some((stem, n)) => Some(free_name(taken, stem, n.saturating_add(1))),
        None if is_generated(current) => Some(free_name(taken, current, 1)),
        None => None,
    }
}

/// The name a fresh node of `kind` should carry among siblings called `taken` —
/// `"Rectangle 1"` in an empty frame, then `"Rectangle 2"`, and the lowest gap
/// after a delete.
///
/// The whole of what `name: None` means on `Operation::CreateNode`.
pub fn created_name(taken: &FxHashSet<&str>, kind: &NodeKind) -> String {
    free_name(taken, kind.default_name(), 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kurbo::Size;

    fn taken<'a>(names: &[&'a str]) -> FxHashSet<&'a str> {
        names.iter().copied().collect()
    }

    /// No wildcard, deliberately: a new `NodeKind` stops compiling here, which
    /// sends the author to `GENERATED_NAMES`; otherwise a missing kind silently
    /// never gets numbered.
    fn listed(kind: &NodeKind) -> bool {
        match kind {
            NodeKind::Root
            | NodeKind::Artboard { .. }
            | NodeKind::Group
            | NodeKind::Rect { .. }
            | NodeKind::Ellipse { .. }
            | NodeKind::Polygon { .. }
            | NodeKind::Star { .. }
            | NodeKind::Line { .. }
            | NodeKind::Path { .. }
            | NodeKind::Boolean { .. }
            | NodeKind::Text { .. } => is_generated(kind.default_name()),
        }
    }

    #[test]
    fn every_default_name_is_listed_as_generated() {
        let one = Size::new(1.0, 1.0);
        let mut kinds = vec![
            NodeKind::Root,
            NodeKind::Artboard { size: one },
            NodeKind::Group,
            NodeKind::Rect {
                size: one,
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            NodeKind::Ellipse { size: one },
            NodeKind::Polygon {
                size: one,
                sides: 3,
                corner_radius: 0.0,
            },
            NodeKind::Star {
                size: one,
                points: 5,
                inner_ratio: 0.5,
                corner_radius: 0.0,
            },
            NodeKind::Line {
                end: crate::kurbo::Point::new(1.0, 0.0),
            },
            NodeKind::Path {
                path: Default::default(),
                corner_radii: Vec::new(),
            },
            NodeKind::Text {
                content: String::new(),
                style: Default::default(),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: crate::node::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
        ];
        kinds.extend(BoolOp::ALL.map(|op| NodeKind::Boolean { op }));
        for kind in &kinds {
            assert!(
                listed(kind),
                "a {:?} is called {:?}, which GENERATED_NAMES does not list — its \
                 layers will never be numbered and its duplicates will never increment",
                kind_label(kind),
                kind.default_name()
            );
        }
    }

    /// A label for the assertion message.
    fn kind_label(kind: &NodeKind) -> String {
        format!("{:?}", std::mem::discriminant(kind))
    }

    /// `numbered` rejects everything but a canonical decimal tail.
    #[test]
    fn only_a_canonical_decimal_tail_reads_as_a_number() {
        assert_eq!(numbered("Rectangle 3"), Some(("Rectangle", 3)));
        assert_eq!(numbered("Badge 12"), Some(("Badge", 12)));
        assert_eq!(numbered("Nav bar 2"), Some(("Nav bar", 2)));
        for odd in [
            "Rectangle",   // no number at all
            "Rectangle 0", // the sequence starts at the bare stem, then 2
            "Layer 007",   // not a decimal we would ever have written
            "Layer +3",
            "Layer 3.0",
            "Layer -1",
            "3",  // no stem
            " 3", // ditto, and the rename field trims anyway
            "Layer 4294967296",
        ] {
            assert_eq!(numbered(odd), None, "{odd:?} parsed as numbered");
        }
    }

    /// Duplicating `Rectangle 4294967295` wraps to a parseable name instead of
    /// overflowing; the parser assertion is what catches the release-mode
    /// `Rectangle 0`.
    ///
    /// flip: `free_name` back to `n += 1` → debug overflow panic, release fails at
    /// "a name the parser can read back".
    #[test]
    fn duplicating_the_last_number_in_the_series_wraps_rather_than_overflowing() {
        let last = format!("Rectangle {}", u32::MAX);
        let copy = copy_name(&taken(&[last.as_str()]), &last).expect("a numbered name is copied");
        assert_ne!(copy, last, "and it is a new name");
        assert!(
            numbered(&copy).is_some(),
            "a name the parser can read back: {copy:?}"
        );
        assert_eq!(
            copy, "Rectangle 1",
            "the lowest gap, which is what a fresh layer gets"
        );

        // One below the top, top taken: the walk reaches `u32::MAX` and wraps.
        let below = format!("Rectangle {}", u32::MAX - 1);
        let copy = copy_name(&taken(&[below.as_str(), last.as_str()]), &below).expect("numbered");
        assert!(numbered(&copy).is_some(), "{copy:?}");
        assert_eq!(copy, "Rectangle 1");
    }

    /// Creating fills the lowest gap; copying starts above the original.
    #[test]
    fn a_new_layer_fills_the_lowest_gap_and_a_copy_lands_above_its_original() {
        assert_eq!(free_name(&taken(&[]), "Rectangle", 1), "Rectangle 1");
        assert_eq!(
            free_name(&taken(&["Rectangle 1"]), "Rectangle", 1),
            "Rectangle 2"
        );
        // The freed number comes back.
        assert_eq!(
            free_name(&taken(&["Rectangle 1", "Rectangle 3"]), "Rectangle", 1),
            "Rectangle 2"
        );
        // A copy of 3 goes to 4 even though 2 is free.
        assert_eq!(
            copy_name(&taken(&["Rectangle 1", "Rectangle 3"]), "Rectangle 3").as_deref(),
            Some("Rectangle 4")
        );
        // And walks up past a taken one.
        assert_eq!(
            copy_name(
                &taken(&["Rectangle 1", "Rectangle 3", "Rectangle 4"]),
                "Rectangle 3"
            )
            .as_deref(),
            Some("Rectangle 5")
        );
        // A bare generated name's copy joins the series from the bottom.
        assert_eq!(
            copy_name(&taken(&["Rectangle"]), "Rectangle").as_deref(),
            Some("Rectangle 1")
        );
        assert_eq!(
            copy_name(&taken(&["Rectangle", "Rectangle 1"]), "Rectangle").as_deref(),
            Some("Rectangle 2")
        );
    }

    /// A typed name survives a copy; a numbered one increments.
    #[test]
    fn a_typed_name_is_kept_on_a_copy_and_a_numbered_one_increments() {
        let names = taken(&["Badge", "Badge 2", "Dot", "Dot"]);
        assert_eq!(copy_name(&names, "Badge"), None, "a typed name is kept");
        assert_eq!(copy_name(&names, "Dot"), None);
        assert_eq!(copy_name(&names, "Nav bar"), None);
        assert_eq!(copy_name(&names, "Badge 2").as_deref(), Some("Badge 3"));
    }

    /// The strict test (overwrite) and the loose one (copy) are told apart.
    #[test]
    fn only_our_own_boolean_names_may_be_overwritten() {
        assert!(is_boolean_label("Union"));
        assert!(is_boolean_label("Subtract 2"));
        for theirs in ["Badge cutout", "Badge 2", "union", "Unions", "Union two"] {
            assert!(
                !is_boolean_label(theirs),
                "{theirs:?} would be renamed out from under the user"
            );
        }
        assert_eq!(
            copy_name(&taken(&["Badge 2"]), "Badge 2").as_deref(),
            Some("Badge 3")
        );
        assert!(generated_stem("Badge 2").is_none());
    }

    /// `usable` answers "is there anything to show", not "is this a good name": a
    /// run of spaces and a punctuation-only name survive.
    #[test]
    fn a_name_is_usable_when_something_is_left_after_trimming() {
        for (raw, want) in [
            ("", None),
            ("   ", None),
            ("\n\t ", None),
            ("Roof", Some("Roof")),
            ("  Roof  ", Some("Roof")),
            ("a\nb", Some("a b")),
            ("a\tb", Some("a b")),
            ("Hello   World", Some("Hello   World")),
            ("...", Some("...")),
        ] {
            assert_eq!(usable(raw).as_deref(), want, "usable({raw:?})");
        }
    }
}
