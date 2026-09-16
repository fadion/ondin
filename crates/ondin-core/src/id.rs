//! Node identity (§5.2, invariant 3).
//!
//! `NodeId = (actor, seq)`: a random per-session `actor` plus a session-local
//! monotonic `seq`. Ids are minted by the caller (`IdSource`) and carried inside
//! operations — `apply` never allocates. This makes op replay deterministic,
//! the property future multiplayer needs. Ids are never reused after deletion.

use serde::{Deserialize, Serialize};

/// Stable, never-reused node identity. See module docs.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct NodeId {
    pub actor: u64,
    pub seq: u64,
}

impl std::fmt::Debug for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Serialized/diagnostic form: "<actor-hex>:<seq>".
        write!(f, "{:x}:{}", self.actor, self.seq)
    }
}

impl NodeId {
    /// Compact, diff-friendly on-disk form: `"<actor-hex>:<seq>"` (§5.2).
    pub fn to_wire(self) -> String {
        format!("{:x}:{}", self.actor, self.seq)
    }

    /// Parse the wire form. Returns `None` on malformed input.
    pub fn from_wire(s: &str) -> Option<NodeId> {
        let (actor, seq) = s.split_once(':')?;
        Some(NodeId {
            actor: u64::from_str_radix(actor, 16).ok()?,
            seq: seq.parse().ok()?,
        })
    }
}

/// Per-session id minter. One is shared by every writer in a session (GUI,
/// `serve`, MCP clients acting through them), so all ids in a session share an
/// `actor` and never collide. A random `actor` avoids cross-session collisions
/// without coordination.
pub struct IdSource {
    actor: u64,
    next_seq: u64,
}

impl IdSource {
    /// Create a source with a caller-provided random actor id.
    ///
    /// Randomness lives outside core (core has no clock/RNG policy); callers
    /// pass a random `u64`. `seq` starts at 1 so `0` is never a valid `seq`.
    pub fn new(actor: u64) -> Self {
        Self { actor, next_seq: 1 }
    }

    /// Mint the next unique id for this session.
    pub fn mint(&mut self) -> NodeId {
        let id = NodeId {
            actor: self.actor,
            seq: self.next_seq,
        };
        self.next_seq += 1;
        id
    }

    pub fn actor(&self) -> u64 {
        self.actor
    }

    /// The seq the next [`mint`](Self::mint) will use. Exposed for diagnostics
    /// and for [`crate::document::reserve_existing_ids`].
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Move the counter forward so the next mint is at least `seq`. Never moves
    /// it backwards — `seq` is monotonic per actor, which is what guarantees a
    /// deleted node's id is never re-minted (§5.2).
    pub fn skip_to(&mut self, seq: u64) {
        self.next_seq = self.next_seq.max(seq);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_is_monotonic_and_unique() {
        let mut src = IdSource::new(0xABCD);
        let a = src.mint();
        let b = src.mint();
        assert_eq!(a.actor, 0xABCD);
        assert_eq!(a.seq, 1);
        assert_eq!(b.seq, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn debug_form_is_actor_hex_colon_seq() {
        let id = NodeId {
            actor: 0x1a2b,
            seq: 42,
        };
        assert_eq!(format!("{id:?}"), "1a2b:42");
    }

    /// **What `from_wire` accepts, pinned** — §15 D662, `[S2.2-L6-08]`.
    ///
    /// 🚨 **This file's two tests covered `mint` and the `Debug` format, and
    /// nothing at all covered the parser.** `NodeId::from_wire` sits behind
    /// `io/schema.rs`'s `parse_id` and therefore behind every node id, every
    /// `parent`, every entry of every `children` list and — through
    /// `GuideId::from_wire` — every guide id in every file the app opens. It is
    /// the one function here that reads **untrusted bytes**, and `tests/io.rs`
    /// exercises it only through well-formed saves of its own making.
    ///
    /// **This pins current behaviour and does not tighten it**, deliberately.
    /// **Five** of the accepted spellings are strings `NodeId::to_wire` can never
    /// produce — a leading `+` on either half, uppercase hex, leading zeroes on
    /// either half — and a sixth, `"0:0"`, names an id the minter guarantees is
    /// never minted (`IdSource::new` starts `seq` at 1; plain backticks because
    /// `cargo doc` cannot see inside a `#[cfg(test)]` module, so a `[link]` here is
    /// decoration no gate can validate — §15 D319). ⚠️ **The eighth row is neither
    /// — `"ffffffffffffffff:1"` is what `to_wire` produces for a `u64::MAX` actor,
    /// so it is a boundary case rather than an alias.** This comment and the test's
    /// own *name* said six until `arch-scribe` counted the table. None is a defect:
    /// every
    /// alias parses to the **same** `NodeId` as its canonical form, so
    /// `into_document`'s duplicate check still catches a file that spells one node
    /// two ways, and a non-canonically spelled cross-reference still resolves.
    ///
    /// ⚠️ **Whether they should be *rejected* is a ruling and is left open.** The
    /// case for it is §5.2's own words — the wire form is *"compact and
    /// diff-**readable**"* — and a diff is only readable if one id has one
    /// spelling. (⚠️ *"diff-friendly"* is `NodeId::to_wire`'s doc one screen up,
    /// and this comment credited it to §5.2 until `arch-scribe` read both; a
    /// quotation attributed to the wrong owner is session 14's `Prefs::ephemeral`
    /// shape and neither gate nor census can see one.) The cost of tightening is
    /// that it narrows what `io::load` accepts, which is a
    /// compatibility decision rather than a repair. The table below is written so
    /// that tightening it is a *visible* edit: move a row from `accepted` to the
    /// rejected list and the intent is on the diff.
    ///
    /// ⚠️ **`"1a2b:"` and `":42"` are already rejected**, which is the control
    /// saying the parser is not simply permissive: an empty half fails
    /// `from_str_radix`/`parse` rather than defaulting to zero.
    #[test]
    fn from_wire_accepts_the_canonical_form_and_five_aliases_of_it() {
        let canonical = NodeId {
            actor: 0x1a2b,
            seq: 42,
        };
        // Every spelling that parses, and what it parses to. The first row is the
        // only one `to_wire` produces; the rest are aliases a hand-edited or
        // foreign file can carry.
        let accepted: [(&str, NodeId); 8] = [
            ("1a2b:42", canonical),
            ("+1a2b:42", canonical),
            ("1A2B:42", canonical),
            ("0001a2b:42", canonical),
            ("1a2b:+42", canonical),
            ("1a2b:042", canonical),
            ("0:0", NodeId { actor: 0, seq: 0 }),
            (
                "ffffffffffffffff:1",
                NodeId {
                    actor: u64::MAX,
                    seq: 1,
                },
            ),
        ];
        for (s, want) in accepted {
            assert_eq!(
                NodeId::from_wire(s),
                Some(want),
                "{s:?} is accepted today; tightening this is a ruling, not a repair"
            );
        }

        // And every spelling that does not. Whitespace is the one worth having: a
        // pretty-printer that indented a JSON string value would otherwise make
        // ids resolve by accident.
        for s in [
            " 1a2b:42",
            "1a2b:42 ",
            "-1:2",
            "1a2b:",
            ":42",
            "1a2b",
            "1a2b:42:1",
            "g:1",
            "1a2b:-1",
            "",
        ] {
            assert_eq!(NodeId::from_wire(s), None, "{s:?} must not parse");
        }

        // The round trip, in the direction that matters: what the writer produces
        // the reader reads back, and spells the same way again.
        for id in [
            canonical,
            NodeId { actor: 0, seq: 1 },
            NodeId {
                actor: u64::MAX,
                seq: u64::MAX,
            },
        ] {
            let wire = id.to_wire();
            assert_eq!(NodeId::from_wire(&wire), Some(id), "{wire} round-trips");
            assert_eq!(
                NodeId::from_wire(&wire).unwrap().to_wire(),
                wire,
                "and re-spells identically, which is what makes a diff readable"
            );
        }
    }

    /// **`skip_to` never moves the counter backwards** — invariant 3's whole
    /// mechanism, and it had no direct test (§15 D662, `[S2.2-L6-08]`).
    ///
    /// A deleted node's id must never be re-minted, and the only thing standing
    /// between the app and that is this one `max`.
    /// `document::reserve_existing_ids` calls it with `seq + 1` for every id in a
    /// loaded file, so a file holding a high id must push the counter past it —
    /// and a file holding a *low* one must not pull it back, which is the half a
    /// plain assignment would break and the half nothing asserted.
    ///
    /// ⚠️ **Flip run**, `self.next_seq = seq` in place of the `max`, and the
    /// predicted site was right: *"a lower seq must not pull the counter back"*,
    /// 2 against 3. The three other assertions stay green under it, which is why
    /// the backwards case has to be here at all — every other thing this function
    /// does is the same under both spellings.
    #[test]
    fn skip_to_only_ever_moves_the_counter_forward() {
        let mut src = IdSource::new(0xABCD);
        assert_eq!(src.next_seq(), 1, "fixture: a fresh source starts at 1");
        src.mint();
        src.mint();
        assert_eq!(src.next_seq(), 3);

        src.skip_to(2);
        assert_eq!(
            src.next_seq(),
            3,
            "a lower seq must not pull the counter back — that is how a deleted \
             node's id gets re-minted"
        );
        src.skip_to(100);
        assert_eq!(src.next_seq(), 100, "and a higher one moves it");
        assert_eq!(
            src.mint().seq,
            100,
            "the next mint is the seq that was asked for"
        );

        // `reserve_existing_ids` passes `seq + 1`, which is what makes an id in
        // the file unreachable rather than merely equalled.
        let mut src = IdSource::new(0xABCD);
        let loaded_seq = 7u64;
        src.skip_to(loaded_seq + 1);
        assert!(
            src.mint().seq > loaded_seq,
            "a live id from the file can never be handed back"
        );
    }
}
