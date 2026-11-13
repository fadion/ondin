//! The single-writer queue (§8.3): every connection's tool calls reach the owning
//! thread here, in the order they were sent, and wait for its answer.
//!
//! A call travels as its name and arguments rather than as a built transaction:
//! a verb reads the document to answer its edit, and the document lives on the
//! owning thread, which commits through the same door as the app's own edits.

use crate::wire::{Handler, Output, Peer};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::time::Duration;

/// How long a call waits for the owning thread to take it. The window drains
/// the queue only between frames, which a modal file dialog stops.
pub const TAKE_WAIT: Duration = Duration::from_secs(30);

const WAITING: u8 = 0;
const TAKEN: u8 = 1;
const ABANDONED: u8 = 2;

/// Which connection a call came from; the owning thread keeps an unkeyed
/// client's current document and revisions under it.
pub type Conn = u64;

/// One tool call waiting for the owning thread.
pub struct Call {
    pub conn: Conn,
    /// The name the client gave at `initialize`.
    pub client: String,
    /// The key it gave there; the host keeps a keyed client's state across its
    /// connections, an unkeyed one's for this connection.
    pub key: Option<String>,
    pub tool: String,
    pub args: Value,
    pub reply: SyncSender<Output>,
    /// Who has the call: the owning thread, which runs it, or the connection,
    /// which gave up waiting. Whichever claims it first decides.
    claim: Arc<AtomicU8>,
}

impl Call {
    /// A call nobody has claimed, for a host's tests.
    pub fn new(
        conn: Conn,
        key: Option<String>,
        tool: &str,
        args: Value,
        reply: SyncSender<Output>,
    ) -> Self {
        Self {
            conn,
            client: "test".to_owned(),
            key,
            tool: tool.to_owned(),
            args,
            reply,
            claim: Arc::default(),
        }
    }

    /// Claim the call for the owning thread; `false` when its connection already
    /// gave up, so it must not run: the agent was told nothing changed.
    pub fn take(&self) -> bool {
        self.claim
            .compare_exchange(WAITING, TAKEN, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// What a call can ask of the owning thread besides a tool: the connection
/// closed, so its state can go.
pub enum Request {
    Call(Call),
    Closed(Conn),
}

/// The owning thread's end of the queue, and the sender each connection clones.
pub fn queue() -> (Sender<Request>, Receiver<Request>) {
    channel()
}

/// A connection's [`Handler`]: forwards each call to the owning thread and waits.
pub struct Forward {
    pub conn: Conn,
    pub to: Sender<Request>,
    /// The tool definitions this host serves.
    pub tools: Vec<Value>,
    /// How long a call waits to be taken: [`TAKE_WAIT`].
    pub wait: Duration,
}

impl Handler for Forward {
    fn tools(&self) -> Vec<Value> {
        self.tools.clone()
    }

    fn call(&mut self, peer: &Peer, name: &str, args: Value) -> Output {
        let (reply, answer) = sync_channel(1);
        let claim = Arc::new(AtomicU8::new(WAITING));
        let call = Call {
            conn: self.conn,
            client: peer.name.clone(),
            key: peer.key.clone(),
            tool: name.to_owned(),
            args,
            reply,
            claim: claim.clone(),
        };
        if self.to.send(Request::Call(call)).is_err() {
            return Output::error("Ondin is closing");
        }
        let dropped = |_| Output::error("Ondin dropped the call");
        match answer.recv_timeout(self.wait) {
            Ok(output) => output,
            Err(RecvTimeoutError::Disconnected) => dropped(()),
            // Not taken yet: give it up, so it never runs after this answer.
            Err(RecvTimeoutError::Timeout)
                if claim
                    .compare_exchange(WAITING, ABANDONED, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok() =>
            {
                Output::error(format!(
                    "Ondin did not take the call within {} s: the window is likely showing a \
                     dialog. Nothing was changed; call again shortly.",
                    self.wait.as_secs()
                ))
            }
            // Taken and running: its answer is on the way.
            Err(RecvTimeoutError::Timeout) => answer.recv().unwrap_or_else(|_| dropped(())),
        }
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        let _ = self.to.send(Request::Closed(self.conn));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A forwarded call reaches the owning end with its connection and client,
    /// the answer comes back, and dropping the connection says it closed.
    #[test]
    fn a_call_is_answered_by_the_owning_end() {
        let (to, from) = queue();
        let owner = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for request in from {
                match request {
                    Request::Call(c) => {
                        assert!(c.take());
                        seen.push(format!("{} {} {}", c.conn, c.client, c.tool));
                        let _ = c.reply.send(Output::json(json!(c.args["n"])));
                    }
                    Request::Closed(conn) => {
                        seen.push(format!("closed {conn}"));
                        break;
                    }
                }
            }
            seen
        });
        let mut f = Forward {
            conn: 7,
            to,
            tools: Vec::new(),
            wait: TAKE_WAIT,
        };
        let peer = Peer {
            name: "agent".into(),
            key: None,
        };
        let out = f.call(&peer, "get_node", json!({ "n": 3 }));
        assert_eq!(out.structured, Some(json!(3)));
        drop(f);
        assert_eq!(owner.join().unwrap(), ["7 agent get_node", "closed 7"]);
    }

    /// A call the owning thread does not take in time is answered as not run,
    /// and the owning thread can no longer take it.
    ///
    /// flip: `Forward::call` answering the timeout without claiming the call →
    /// fails at "too late to run".
    #[test]
    fn a_call_not_taken_in_time_never_runs() {
        let (to, from) = queue();
        let mut f = Forward {
            conn: 1,
            to,
            tools: Vec::new(),
            wait: Duration::from_millis(20),
        };
        let out = f.call(&Peer::default(), "set_name", json!({}));
        assert!(out.is_error, "answered as not taken");
        let Ok(Request::Call(late)) = from.try_recv() else {
            panic!("the call was queued");
        };
        assert!(!late.take(), "too late to run");
    }
}
