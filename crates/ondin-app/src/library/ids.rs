//! Library ids: the opaque strings a document and a project are known by.
//!
//! **Not `NodeId`, and not a slug.** `NodeId` is minted per session from a
//! random actor and is a document's *internal* vocabulary; a slug is derived
//! from a name and therefore changes when the name does. What the library needs
//! is neither: a document's id keys its version history and its thumbnail, and a
//! project's id is written into every one of its files — so both have to survive
//! a rename, and neither may ever be re-derived from anything the user can edit.
//!
//! **Random rather than sequential**, because the base folder can be shared: two
//! machines syncing into one Dropbox directory both mint ids, with no way to
//! coordinate and no chance to notice a collision before it has been written
//! into a dozen files. 128 bits of `SystemRandom` makes that not a thing anyone
//! has to think about again, where a counter would need a merge rule.

use ring::rand::{SecureRandom, SystemRandom};

/// The length of a minted id in hex characters — 128 bits.
const ID_HEX_LEN: usize = 32;

/// A fresh, opaque id: 32 lowercase hex characters.
///
/// ⚠️ **Falls back to the clock if the OS random source refuses**, rather than
/// panicking or returning an error every caller would have to answer. A failing
/// `SystemRandom` is close to impossible and the consequence of the fallback is
/// bounded — two ids minted in the same nanosecond on two machines collide,
/// where the random ones would not — so the trade is a vanishingly rare weaker
/// id against a *Create* button that can fail. The prefix makes such an id
/// recognisable in a file if it ever does happen.
pub fn mint() -> String {
    let mut bytes = [0u8; ID_HEX_LEN / 2];
    if SystemRandom::new().fill(&mut bytes).is_ok() {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("f0{nanos:030x}")
}

/// Whether `s` is an id this app minted — 32 lowercase hex characters.
///
/// **The belt to `ondin_core::DocumentMeta::sanitized`'s braces**, and the two
/// are deliberately not the same line of code doing the same job twice. Core
/// drops a malformed id at the *boundary*, where a file's bytes become a
/// document, and that closes the class. This is what the three writers that join
/// an id as a **path component** ask before they join it — the crash snapshot,
/// the version pin and the cover cache — so that a fourth route into one of them
/// (a hand-built `Entry`, a future reader that does not go through
/// `io::load` or `io::probe`) refuses rather than escaping the library folder.
///
/// ⚠️ **That fourth route is not hypothetical: there were two boundaries, not
/// one.** `library::scan::read_meta` answers from `io::probe::meta_in_prefix`
/// and never loads the document at all, and it is the reader behind
/// `cover::key` — the path that runs for every card the dashboard draws, with
/// nothing clicked. It was found by grepping the field rather than by reading
/// the load path.
pub fn is_minted(s: &str) -> bool {
    ondin_core::DocumentMeta::is_wellformed_id(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn an_id_is_thirty_two_lowercase_hex_characters() {
        let id = mint();
        assert_eq!(id.len(), ID_HEX_LEN, "{id}");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "{id}"
        );
    }

    /// ⚠️ **Also asserts the length of the fallback**, which is the branch no
    /// test can reach by making `SystemRandom` fail: `f0` plus 30 hex digits of
    /// nanoseconds is 32 characters only if the format width is right, and a
    /// nanosecond count needs 30 hex digits' room for roughly forever. Getting
    /// that wrong produces an id of the wrong length in the one situation nobody
    /// is watching.
    #[test]
    fn the_clock_fallback_is_the_same_shape_as_a_random_id() {
        let nanos: u128 = 1_774_483_200_000_000_000;
        let fallback = format!("f0{nanos:030x}");
        assert_eq!(fallback.len(), ID_HEX_LEN, "{fallback}");
        assert!(
            fallback.chars().all(|c| c.is_ascii_hexdigit()),
            "{fallback}"
        );
    }

    /// Not a proof of uniqueness — nothing here could be — but it does catch the
    /// mistake this function is most likely to make, which is returning a
    /// constant or reusing one buffer.
    #[test]
    fn minting_repeatedly_does_not_repeat() {
        let ids: HashSet<String> = (0..256).map(|_| mint()).collect();
        assert_eq!(ids.len(), 256);
    }
}
