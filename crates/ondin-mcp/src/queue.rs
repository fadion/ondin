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

/// A document-level action, the queue's second request kind (§8.3, invariant
/// 10, §15 D1097): what a person does *to* a document that is not an edit *of*
/// it. None is a `Transaction`, so none enters the history; the owning thread
/// runs the app's own code for each and replies with its result. On the same
/// queue as [`OpRequest`] so it is ordered against the edits around it — a
/// publish sent after a batch of writes publishes those writes.
pub enum DocAction {
    /// Mark the open document a kit.
    MakeKit,
    /// Publish the open kit with this note.
    PublishKit { note: String },
    /// Stop sharing the open kit; consumers keep their copies.
    StopSharingKit,
    /// File a new document in the library, optionally in a project.
    CreateDocument {
        name: String,
        project: Option<String>,
    },
}

/// A document-level action placed on the queue. The `reply` half is added
/// with [`OpRequest`]'s at M6.
pub struct ActionRequest {
    pub action: DocAction,
}
