//! Layer names the app generates, and how they get their numbers (§15 D127).
//!
//! Every create site passes `name: None` and `Document::op_create` turns that
//! into a name. Without numbering that made ten rectangles all literally
//! "Rectangle" — which matters more here than in Figma, because `snapshot.rs`
//! emits `node.name()` into the AI-handoff projection and Command Mode (§13)
//! addresses layers by name as a fallback. Ten identical names leave both
//! ambiguous.
//!
//! **Uniqueness is never load-bearing, and this module is written so it cannot
//! become so.** Nothing resolves a node *by* name — every lookup in the
//! workspace is by `NodeId`, §13 states outright that "lookup by layer name is a
//! disambiguated fallback, never the primary mechanism (names aren't unique)",
//! and the snapshot carries `id`, `parent` and `children` beside the name so a
//! consumer can disambiguate by path or ignore names entirely. So the numbering
//! here is a *courtesy*: it makes the common case readable, and every collision
//! it declines to prevent is legal.
//!
//! **Two rules, and they answer different questions.** The app owns the names it
//! generated; the user owns the ones they typed. So:
//!
//! - creating numbers within the parent, from the generated stem for that kind,
//!   starting at 1 — so it is "Rectangle 1", "Rectangle 2", and never a bare
//!   "Rectangle" beside a numbered one ([`free_name`]);
//! - copying keeps the name **unless** it looks numbered, in which case it
//!   increments ([`copy_name`]).
//!
//! Neither ever runs again afterwards. Reparenting a layer into a frame that
//! already holds a "Rectangle 2" is allowed to collide, because recomputing
//! names on move would rename things behind the user's back — which is worse
//! than a repeated name in a panel that shows the tree anyway.
//!
//! There is no stored "this name is auto" flag, and no persisted per-kind
//! counter. Both facts are read back out of the names currently present, which
//! is what makes freed numbers reusable and what stops the numbering silently
//! resetting on reload (a counter would have to be saved, and a counter that is
//! not saved is worse than none).

use crate::node::{BoolOp, NodeKind};
use rustc_hash::FxHashSet;

/// Every name [`NodeKind::default_name`] can produce.
///
/// **A list rather than a walk over the variants**, because the question asked of
/// it is the inverse one — "would we have written this string" — and a `match` on
/// a `NodeKind` cannot answer that from a name alone. It is checked against
/// `default_name` by [`every_default_name_is_listed_as_generated`], so a new kind
/// cannot slip past: adding one without adding it here fails that test rather than
/// quietly making its name un-numberable.
///
/// [`every_default_name_is_listed_as_generated`]: self
pub const GENERATED_NAMES: &[&str] = &[
    "Root",
    // "Frame", not "Artboard" — §15 D21: the type is `Artboard` and every word
    // the user reads is frame.
    "Frame",
    "Group",
    "Rectangle",
    "Ellipse",
    "Polygon",
    "Star",
    "Line",
    "Path",
    "Text",
    // The four boolean labels, which are `default_name`'s output for
    // `NodeKind::Boolean` — named for the operation, as Figma does.
    "Union",
    "Subtract",
    "Intersect",
    "Exclude",
];

/// Whether `name` is a **stem** this app numbers from, as opposed to a word a
/// person typed.
///
/// The stem alone is not a name the app writes any more — every generated name
/// carries its number ([`free_name`]) — so this is asked about the part in front
/// of one, and about a bare name only where a document saved before the numbering
/// might still carry it.
///
/// Exact, and deliberately so: "Rectangle" is our stem and "Rectangles" is not.
pub fn is_generated(name: &str) -> bool {
    GENERATED_NAMES.contains(&name)
}

/// A caller-supplied name reduced to one this app can show, or `None` when there
/// is nothing usable in it (§15 D644, `[S2.2-L1-07]`).
///
/// **The rule already existed on two surfaces and there was a third that agreed
/// with neither.** `layers::layer_rename_field` and `inspector`'s name field both
/// do `buf.trim()` and refuse an empty result, the second with its reason
/// written beside it: *"selecting all and pressing Delete here left a row with
/// no label at all, where doing the same thing in the tree two panels away did
/// nothing. Two surfaces for one edit have to agree on what a name is."* SVG
/// import was the third, and it installed `""` and `"   "` verbatim — measured
/// end to end, a blank row in the layers panel with no label at all.
///
/// ⚠️ **That quotation is §15 D127's and this doc credited it to D54**, which
/// `arch-scribe` caught by reading the citation against the entry. Both numbers
/// resolve, so no gate in this project could ever have seen it. D54 is the
/// layers panel's entry and says only *"a name typed empty is discarded too"*;
/// the sentence above is the inspector field's and cites D127 on its own line.
///
/// ⚠️ **One home and two copies, not a unification.** Both fields still
/// hand-roll their `trim`/`is_empty`, and nothing stops them calling this —
/// `naming` is a `pub mod`. Until they do, the three surfaces still differ in
/// the small: only import normalises interior whitespace, so a name holding a
/// tab survives a rename field and would not survive a round trip through SVG.
///
/// ⚠️ **Returning `None` rather than a repaired string is the whole point at the
/// import site**: `op_create` turns `None` into a generated name, which is
/// exactly what an element with no `id` already gets, so an unusable name falls
/// through to the answer the document already has for "unnamed".
///
/// **Every interior whitespace character becomes one ordinary space** — a no-op
/// for a space itself, and what turns `"a\nb"` into `"a b"` — because the layers
/// panel lays out one line and a newline in it is not a name, it is a break. An
/// SVG `id` cannot contain whitespace at all (XML forbids it in a `Name`), so
/// this only ever fires on `data-name` or on a file no validator would pass.
///
/// ⚠️ **Runs are not collapsed.** `"Hello   World"` keeps its three spaces:
/// somebody typed that, and squeezing it would be inventing an intent the way
/// [`numbered`] declines to read `"Layer 007"` as a number.
///
/// ⚠️ **This is not a uniqueness rule and must not become one.** §15 D127's
/// recorded non-goal is that uniqueness must never become load-bearing; this is
/// about whether a name is *displayable*, which is the question D54 and D127
/// answered for the two fields. Nothing in the record ever said a name may be
/// empty — §15 D644 is the first entry to state that it may not.
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
/// **The tail has to be the canonical decimal it parses back to**, which is what
/// rejects `"Layer 007"`, `"Layer +3"` and `"Layer 3 "`. A name we did not write
/// is the user's, and `"007"` is not a number this module has ever produced, so
/// reading it as one would be inventing an intent.
///
/// Zero is rejected for the same reason: the sequence starts at 1, so
/// `"Rectangle 0"` was typed by hand.
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
/// **Narrower than [`copy_name`]'s test, and the difference is the point.**
/// Copying increments anything that *looks* numbered, because someone who typed
/// "Badge 2" and duplicated it wanted "Badge 3" — a benign false positive. This
/// asks the strict question instead: is this string one we would have generated,
/// so may we overwrite it? `build::set_boolean_op` needs that one, because
/// getting it wrong means silently renaming a layer the user named. The two must
/// not be unified.
pub fn generated_stem(name: &str) -> Option<&str> {
    let stem = numbered(name).map_or(name, |(stem, _)| stem);
    is_generated(stem).then_some(stem)
}

/// Whether `name` is the app's own name for a boolean of some operation —
/// `build::set_boolean_op`'s "may I rename this" test.
///
/// Here rather than in `build` so it reads against [`generated_stem`], whose doc
/// has why this is the strict question and `copy_name`'s is the loose one.
pub fn is_boolean_label(name: &str) -> bool {
    generated_stem(name).is_some_and(|stem| BoolOp::ALL.iter().any(|op| op.label() == stem))
}

/// The first name in the sequence `stem <from>`, `stem <from+1>`, … that `taken`
/// does not already hold.
///
/// **Every generated name carries a number, the first one included.** A first
/// rectangle called "Rectangle" beside a second called "Rectangle 2" reads as
/// though the first one were somehow special — reported exactly that way — and it
/// costs the layers panel its symmetry for nothing. So the series starts at
/// "Rectangle 1" and `from` is always at least 1.
///
/// **The first free number, not the next one after the highest**, which is
/// Figma's reset-on-delete and needs no stored state: delete "Rectangle 2" and
/// the next rectangle is called "Rectangle 2" again. The objection that a reused
/// name now refers to a different object is mild here, because the file's
/// identity is the string node id — a diff shows a new id carrying a reused name
/// and reads correctly.
///
/// **`from` is what makes copying different from creating.** A create starts at 1
/// and takes the lowest gap. A copy of "Rectangle 3" starts at 4, so it lands
/// above its original rather than in whatever gap is lowest — "Badge 2" duplicated
/// is "Badge 3", which is what was asked for.
///
/// ⚠️ **The search wraps to 1 at `u32::MAX`, and that is not belt-and-braces**
/// (§15 D433).
/// This doc used to argue the bound away — *"a parent would need four billion
/// children to exhaust it"* — which is true of [`created_name`], whose `from` is
/// always 1, and **false of [`copy_name`]**, whose `from` is parsed out of a
/// string the user typed. Rename a layer to `Rectangle 4294967295` and duplicate
/// it and the loop needs *one* iteration to overflow: a debug panic out of core
/// on the UI thread mid-gesture, and in release a `Rectangle 0` — a name
/// [`numbered`] documents as impossible and refuses to parse, so the copy is
/// mis-read as hand-typed from then on.
///
/// **Wrapping to the lowest gap rather than failing**, because that is
/// `created_name`'s own rule and it produces a name in the series. A copy would
/// rather sit just above its original, but at `u32::MAX` there is no "just
/// above" to have, and the next best answer is the one a fresh layer would get.
///
/// The final line is reached only if all four billion names are taken, which no
/// `taken` that fits in memory can be; it answers a duplicate, which is legal
/// here (nothing requires sibling names to differ) rather than a name the parser
/// rejects.
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
/// **A duplicate keeps its name, as Figma's does, except when the name is
/// numbered.** "Badge" duplicated is "Badge" — ten layers called "Dot" in a chart
/// is correct, and renaming them would be hostile. But "Rectangle 2" duplicated
/// is "Rectangle 3", because a name carrying a number is a name in a series and
/// the copy belongs after it.
///
/// **Pattern-matched rather than flagged.** An "is auto" bit would have to live in
/// the *model* to survive being saved, and the false positive it would buy is
/// benign: someone who typed "Badge 2" by hand and duplicates it gets "Badge 3",
/// which is what they wanted. The same argument that rejected a stored
/// point-type flag on `PenAnchor` (§15 D114), reached from the other end.
///
/// **An *unnumbered* generated name joins the series from the bottom.** Nothing
/// writes one any more — every generated name is numbered now — so a bare
/// "Rectangle" is either a name from a document saved before that or one somebody
/// typed to look like ours, and either way its copy belongs in the series rather
/// than beside it as a second bare "Rectangle".
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

    /// **No wildcard, deliberately.** A new `NodeKind` stops compiling *here*, and
    /// that is the whole value of this function: the arm it needs is one line, and
    /// arriving at it is what sends the author to `GENERATED_NAMES`.
    ///
    /// Without it the list drifts in silence. `GENERATED_NAMES` is the *inverse* of
    /// `default_name` — "would we have written this string" cannot be answered by a
    /// `match` on a kind — so a kind missing from it compiles, saves and loads
    /// perfectly, and the only symptom is that one kind's layers never get numbered
    /// and its duplicates never increment. That is the `matches!`-over-a-list trap
    /// the boolean kind was caught by four times over (§15 D87–D88), in the one
    /// shape it can still take.
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
            },
            NodeKind::Ellipse { size: one },
            NodeKind::Polygon {
                size: one,
                sides: 3,
            },
            NodeKind::Star {
                size: one,
                points: 5,
                inner_ratio: 0.5,
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

    /// A name for the assertion above that is not the one under test.
    fn kind_label(kind: &NodeKind) -> String {
        format!("{:?}", std::mem::discriminant(kind))
    }

    /// The parser has to reject everything it did not write, because anything else
    /// is the user's and incrementing it would be inventing an intent.
    #[test]
    fn only_a_canonical_decimal_tail_reads_as_a_number() {
        assert_eq!(numbered("Rectangle 3"), Some(("Rectangle", 3)));
        assert_eq!(numbered("Badge 12"), Some(("Badge", 12)));
        // A stem with a space of its own is still a stem.
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

    /// **`[S2.2-L1-01]`'s loss: `Ctrl+D` on a layer called `Rectangle 4294967295`.**
    ///
    /// `copy_name` starts the search at `n.saturating_add(1)`, which at
    /// `u32::MAX` saturates to a number that is *already taken* — the original's
    /// own name, which `CopyNames::rename` puts in `taken` because
    /// `child_names(parent, &[])` excludes nothing. So `free_name` found it and
    /// added one: **an overflow panic out of core on the UI thread during an
    /// ordinary duplicate**, taking whatever was unsaved with it. Release did not
    /// panic and answered `"Rectangle 0"`, a name `numbered` refuses to parse,
    /// so the copy was mis-classified as hand-typed for the rest of its life.
    ///
    /// ⚠️ **The panic is debug-only and therefore invisible to the release gate**,
    /// and the release answer is invisible to everything — it is a plausible
    /// string. This test asserts *both*: that the call returns, and that what it
    /// returns is a name the parser accepts.
    ///
    /// The second fixture is the reachable spelling that needs no `u32::MAX` in
    /// the *typed* name: a sibling one above it.
    ///
    /// Flip-check, run: taking the wrap out of `free_name` — back to `n += 1` —
    /// fails here in debug with *"attempt to add with overflow"* at
    /// `naming.rs`, and in release at *"a name the parser can read back"*, which
    /// is why the parser assertion is not decoration.
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

        // One below the top, with the top already taken: the walk reaches
        // `u32::MAX`, finds it taken, and wraps.
        let below = format!("Rectangle {}", u32::MAX - 1);
        let copy = copy_name(&taken(&[below.as_str(), last.as_str()]), &below).expect("numbered");
        assert!(numbered(&copy).is_some(), "{copy:?}");
        assert_eq!(copy, "Rectangle 1");
    }

    /// Creating fills the lowest gap; copying starts above the original. Both
    /// halves matter and they are one argument apart.
    #[test]
    fn a_new_layer_fills_the_lowest_gap_and_a_copy_lands_above_its_original() {
        // **The first of a kind is numbered too.** A bare "Rectangle" beside a
        // "Rectangle 2" reads as though the first were special, and it was reported
        // that way — so nothing generated is ever unnumbered.
        assert_eq!(free_name(&taken(&[]), "Rectangle", 1), "Rectangle 1");
        assert_eq!(
            free_name(&taken(&["Rectangle 1"]), "Rectangle", 1),
            "Rectangle 2"
        );
        // The freed number comes back, which is what needs no stored counter.
        assert_eq!(
            free_name(&taken(&["Rectangle 1", "Rectangle 3"]), "Rectangle", 1),
            "Rectangle 2"
        );
        // A copy of 3 goes to 4 even though 2 is free — it belongs after the thing
        // it was copied from.
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
        // An *unnumbered* generated name can only come from a document saved before
        // the numbering, or from someone typing one of our stems. Its copy joins the
        // series from the bottom rather than sitting beside it as a second bare one.
        assert_eq!(
            copy_name(&taken(&["Rectangle"]), "Rectangle").as_deref(),
            Some("Rectangle 1")
        );
        assert_eq!(
            copy_name(&taken(&["Rectangle", "Rectangle 1"]), "Rectangle").as_deref(),
            Some("Rectangle 2")
        );
    }

    /// **The name the user typed survives being copied**, which is the half of the
    /// rule that has to hold or the feature is hostile: ten layers called "Dot" in
    /// a chart is correct.
    #[test]
    fn a_typed_name_is_kept_on_a_copy_and_a_numbered_one_increments() {
        let names = taken(&["Badge", "Badge 2", "Dot", "Dot"]);
        assert_eq!(copy_name(&names, "Badge"), None, "a typed name is kept");
        assert_eq!(copy_name(&names, "Dot"), None);
        assert_eq!(copy_name(&names, "Nav bar"), None);
        // Numbered, so it is in a series and the copy goes after it — the benign
        // false positive the pattern-match buys.
        assert_eq!(copy_name(&names, "Badge 2").as_deref(), Some("Badge 3"));
    }

    /// The strict test and the loose one, told apart. `set_boolean_op` may only
    /// overwrite a name the app wrote; a duplicate may increment anything numbered.
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
        // The loose test is loose on exactly the case the strict one refuses.
        assert_eq!(
            copy_name(&taken(&["Badge 2"]), "Badge 2").as_deref(),
            Some("Badge 3")
        );
        assert!(generated_stem("Badge 2").is_none());
    }

    /// The predicate the two rename fields have always applied, now written down
    /// once (§15 D644, `[S2.2-L1-07]`).
    ///
    /// ⚠️ **The last two rows are the ones that keep this from becoming a
    /// cleaner-upper.** A run of spaces inside a name survives, and so does a
    /// name that is nothing but punctuation: this answers *"is there anything to
    /// show"*, not *"is this a good name"*, and the second question is not one
    /// this crate is entitled to.
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
