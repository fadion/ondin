//! Native document IO (§5.11, invariant 9).
//!
//! Native format = JSON. Saves are deterministic (nodes emitted sorted by
//! `NodeId`, stable field order) so re-saving is byte-identical. Loading reads
//! `schema_version` and runs the migration chain up to current.
//!
//! ⚠️ **The version is [`CURRENT_SCHEMA_VERSION`] and this line does not repeat
//! it.** It read "JSON v1" until 2026-09-06, four bumps stale, *directly above
//! the constant that defines it* — and `architecture.md` §5.11 said "currently
//! v3" at the same moment, which is the same number written in three places and
//! wrong in two. A number a reader can get from the item eight lines down is a
//! number this comment has no business carrying.

pub mod clip;
pub mod migrate;
pub mod probe;
pub mod schema;

use crate::document::Document;

/// Current on-disk schema version, written on every save.
pub const CURRENT_SCHEMA_VERSION: u32 = 4;

/// How deep a document's tree may be nested, root at 0 (§15 D416).
///
/// **A stack bound, not a taste bound**, and the two numbers it sits between are
/// what chose it. Measured on this machine: a nested-group document overflows
/// the stack inside [`crate::Resolved::rebuild`] at **1,000** levels in a debug
/// build and at **2,000** in release — and a stack overflow is not a panic, so
/// nothing catches it, nothing reports it, and the session's unsaved work goes
/// with the process. Against that, the deepest thing anyone builds by hand is a
/// handful of nested groups. 256 is two orders under the abort and far past
/// anything legitimate, which is the shape a refusal on the load path has to
/// have: it can lock a user out of a real document, so it must be generous.
///
/// Enforced in two places, because there are two doors: check (6) of
/// `schema::verify_integrity` — plain backticks, because it is private to that
/// module and a `[link]` here is one no gate can resolve — for a `.ondin` file,
/// and
/// [`crate::svg_in`]'s element recursion for a pasted or imported SVG — which is
/// the shallower door of the two, aborting at depth 200 in a debug build, and in
/// that module's own words *"as untrusted as input gets"*.
///
/// ⚠️ **The operation layer does not enforce it**, so a document built by
/// repeated `CreateNode` can still exceed what the loader will read back. That
/// is the review's G1 class — a value the op layer accepts and the loader
/// rejects — and it is recorded rather than closed here: unlike the two doors
/// above, no measured route reaches it (nothing in the app nests without a user
/// click per level).
pub const MAX_TREE_DEPTH: usize = 256;

#[derive(thiserror::Error, Debug)]
pub enum IoError {
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    /// The version the *file* claims, not a truncation of it (§15 D638). It is a
    /// `u64` because that is what `serde_json` can hold in the header and the
    /// range check has to happen before any narrowing — `8589934592` cast to
    /// `u32` is `0`, which is a version this loader migrates rather than refuses.
    #[error("unsupported schema version: {0}")]
    UnsupportedVersion(u64),
    #[error("document integrity error: {0}")]
    Integrity(String),
}

/// Serialize to native JSON bytes.
///
/// Deterministic (invariant 9): nodes are emitted sorted by id and every field
/// has a fixed order, so saving the same document twice yields identical bytes.
/// Pretty-printed for git-friendly diffs.
pub fn save(doc: &Document) -> Result<Vec<u8>, IoError> {
    let dto = schema::DocumentDto::from_document(doc);
    let mut bytes = serde_json::to_vec_pretty(&dto)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Parse native bytes: read `schema_version`, migrate to current, then build
/// and verify the document.
pub fn load(bytes: &[u8]) -> Result<Document, IoError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let migrated = migrate::migrate(value)?;
    let dto: schema::DocumentDto = serde_json::from_value(migrated)?;
    dto.into_document()
}
