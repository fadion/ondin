//! ondin-mcp — the MCP adapter (§8, invariant 10).
//!
//! A thin adapter: tool handlers only translate to/from `Operation`s and
//! queries; all logic lives in core. The owning process (GUI or `serve`) holds
//! the document; mutations funnel through a single-writer op queue. This crate
//! has no rendering or UI dependency.
//!
//! M0 scaffold: op-queue message types and the tool list are sketched; the
//! socket listener and stdio proxy land at M6.

// ⚠️ **`#![allow(dead_code)]` was here and is gone** (§15 D732, `[A8-L3-06]`) —
// see `ondin_export`'s crate root, which carries the argument. It was hiding
// nothing, and it mattered more here than there: this crate is *"an M0 scaffold"*
// whose whole content is message types and a tool list waiting for M6, so it is
// precisely the crate where an item can quietly lose its last caller and where
// nobody would be reading. **A lint switched off in a crate nobody opens is a
// lint switched off for as long as that lasts.**
//
// The rustdoc gate, and why one of the three lints is allowed: see `ondin_core`'s
// crate doc (§15 D296).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

pub mod queue;
pub mod tools;
