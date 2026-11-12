//! ondin-mcp — the MCP adapter (§8, invariant 10).
//!
//! The wire (JSON-RPC over lines), the tool definitions, and the queue that
//! carries each call to the thread owning the documents. Handlers live with the
//! owner (`ondin-app`'s `mcp` module), which commits through the same door as the
//! app's own edits. This crate has no rendering or UI dependency.

// The rustdoc gate, and why one of the three lints is allowed: see `ondin_core`'s
// crate doc (§15 D296).
#![deny(rustdoc::broken_intra_doc_links, rustdoc::invalid_html_tags)]
#![allow(rustdoc::private_intra_doc_links)]

pub mod queue;
pub mod schema;
pub mod tools;
pub mod wire;
