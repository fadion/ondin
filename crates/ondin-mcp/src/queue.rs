//! The single-writer op queue (§8.3).
//!
//! MCP handlers build `Transaction`s with the session `IdSource` and submit
//! them here; the owning thread applies them through the same path as UI edits,
//! preserving one source of truth and one shared history. The concrete channel
//! (`mpsc` + `oneshot`) is wired at M6; this sketches the request shape.

use ondin_core::{ApplyOutcome, OpError, Transaction};

/// A mutation request placed on the queue. The `reply` half (a oneshot sender)
/// is added when the transport is wired at M6.
pub struct OpRequest {
    pub tx: Transaction,
    // pub reply: oneshot::Sender<Result<ApplyOutcome, OpError>>,  // M6
}

/// Marker for the reply payload the owning thread produces.
pub type OpReply = Result<ApplyOutcome, OpError>;
