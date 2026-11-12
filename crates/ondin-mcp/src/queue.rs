//! The single-writer queue (§8.3): every connection's tool calls reach the owning
//! thread here, in the order they were sent, and wait for its answer.
//!
//! A call travels as its name and arguments rather than as a built transaction:
//! a verb reads the document to answer its edit, and the document lives on the
//! owning thread, which commits through the same door as the app's own edits.

use crate::wire::{Handler, Output};
use serde_json::Value;
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};

/// Which connection a call came from; the owning thread keeps each one's
/// current document and last revision under it.
pub type Conn = u64;

/// One tool call waiting for the owning thread.
pub struct Call {
    pub conn: Conn,
    /// The name the client gave at `initialize`.
    pub client: String,
    pub tool: String,
    pub args: Value,
    pub reply: SyncSender<Output>,
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
}

impl Handler for Forward {
    fn tools(&self) -> Vec<Value> {
        self.tools.clone()
    }

    fn call(&mut self, client: &str, name: &str, args: Value) -> Output {
        let (reply, answer) = sync_channel(1);
        let call = Call {
            conn: self.conn,
            client: client.to_owned(),
            tool: name.to_owned(),
            args,
            reply,
        };
        if self.to.send(Request::Call(call)).is_err() {
            return Output::error("Ondin is closing");
        }
        answer
            .recv()
            .unwrap_or_else(|_| Output::error("Ondin dropped the call"))
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
        };
        let out = f.call("agent", "get_node", json!({ "n": 3 }));
        assert_eq!(out.structured, Some(json!(3)));
        drop(f);
        assert_eq!(owner.join().unwrap(), ["7 agent get_node", "closed 7"]);
    }
}
