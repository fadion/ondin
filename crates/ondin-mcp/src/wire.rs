//! The MCP wire (§8.1a): JSON-RPC 2.0, one message per line, read and written on
//! the connection's own thread.
//!
//! Only what a tool server needs: `initialize`, `ping`, `tools/list`,
//! `tools/call`, and notifications, which are read and dropped (a `cancelled`
//! arrives after its call has already answered, since calls run in order).

use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// The protocol revisions this server speaks, newest first. A client asking for
/// one of them gets it; any other gets the newest.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The text `initialize` hands the client for its model: how to pick a document.
pub const INSTRUCTIONS: &str = "Ondin edits design documents. Call `use_document` with the \
file the person names before anything else; every later call acts on that document until \
`use_document` is called again, which is only when the person names another file. Every \
result carries `rev`, and `changed` when the document moved since you last looked: what the \
person or an undo changed. Reach a part of a page with `find_nodes` and `get_node`, not \
`get_document`. `export_png` shows what the page looks like.";

/// One piece of a tool's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    /// Encoded bytes and their media type.
    Image {
        bytes: Vec<u8>,
        mime: &'static str,
    },
}

/// A tool's answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub content: Vec<Content>,
    /// The same answer as one JSON value, for a client that reads structure.
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl Output {
    /// An answer that is one JSON value, also given as its text.
    pub fn json(value: Value) -> Self {
        Self {
            content: vec![Content::Text(value.to_string())],
            structured: Some(value),
            is_error: false,
        }
    }

    /// A refusal the model reads and can act on.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text(message.into())],
            structured: None,
            is_error: true,
        }
    }
}

/// What answers a connection's tool calls.
pub trait Handler {
    /// The tool definitions `tools/list` answers.
    fn tools(&self) -> Vec<Value>;
    /// Answer one call; `client` is the name the client gave at `initialize`.
    fn call(&mut self, client: &str, name: &str, args: Value) -> Output;
}

/// Answer requests read from `read` on `write` until the reader ends.
pub fn serve(
    read: impl BufRead,
    mut write: impl Write,
    handler: &mut impl Handler,
) -> std::io::Result<()> {
    let mut client = String::new();
    for line in read.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = answer(&line, &mut client, handler) {
            writeln!(write, "{reply}")?;
            write.flush()?;
        }
    }
    Ok(())
}

/// The reply to one line, `None` for a notification.
fn answer(line: &str, client: &mut String, handler: &mut impl Handler) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(failure(Value::Null, -32700, &format!("not JSON: {e}"))),
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response to nothing this server asked, or a batch, which this
        // protocol no longer has.
        let id = message.get("id").cloned()?;
        return Some(failure(id, -32600, "expected a request object"));
    };
    let id = message.get("id").cloned()?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            if let Some(name) = params.pointer("/clientInfo/name").and_then(Value::as_str) {
                name.clone_into(client);
            }
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked
                .filter(|v| PROTOCOL_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSIONS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": { "name": "ondin", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": handler.tools() }),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(failure(id, -32602, "tools/call needs `name`"));
            };
            let args = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(a) => a.clone(),
            };
            result_of(handler.call(client, name, args))
        }
        other => return Some(failure(id, -32601, &format!("no method `{other}`"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// `tools/call`'s result for an answer.
fn result_of(output: Output) -> Value {
    use base64::Engine as _;
    let content: Vec<Value> = output
        .content
        .into_iter()
        .map(|c| match c {
            Content::Text(text) => json!({ "type": "text", "text": text }),
            Content::Image { bytes, mime } => json!({
                "type": "image",
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                "mimeType": mime,
            }),
        })
        .collect();
    let mut result = json!({ "content": content, "isError": output.is_error });
    if let Some(structured) = output.structured {
        result["structuredContent"] = structured;
    }
    result
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    impl Handler for Echo {
        fn tools(&self) -> Vec<Value> {
            vec![json!({ "name": "echo" })]
        }
        fn call(&mut self, client: &str, name: &str, args: Value) -> Output {
            Output::json(json!({ "client": client, "name": name, "args": args }))
        }
    }

    fn run(input: &str) -> Vec<Value> {
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, &mut Echo).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// A session's handshake, a call and a notification answer in order, the
    /// notification with nothing, and the call sees the client's name.
    #[test]
    fn a_session_answers_requests_and_drops_notifications() {
        let replies = run(concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"claude-code"}}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"a":1}}}"#,
            "\n",
        ));
        assert_eq!(replies.len(), 2);
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(replies[1]["id"], 2);
        let s = &replies[1]["result"]["structuredContent"];
        assert_eq!(s["client"], "claude-code");
        assert_eq!(s["args"]["a"], 1);
    }

    /// An unknown protocol revision gets the newest; an unknown method and a
    /// line that is not JSON get JSON-RPC errors.
    #[test]
    fn what_the_server_does_not_know_is_answered_with_an_error() {
        let replies = run(concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"1999-01-01"}}"#,
            "\nnot json\n",
            r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
            "\n",
        ));
        assert_eq!(
            replies[0]["result"]["protocolVersion"],
            PROTOCOL_VERSIONS[0]
        );
        assert_eq!(replies[1]["error"]["code"], -32700);
        assert_eq!(replies[2]["error"]["code"], -32601);
    }

    /// An image answer travels as base64 with its media type.
    #[test]
    fn an_image_is_sent_as_base64() {
        let output = Output {
            content: vec![Content::Image {
                bytes: vec![0xff, 0x00],
                mime: "image/png",
            }],
            structured: None,
            is_error: false,
        };
        let r = result_of(output);
        assert_eq!(r["content"][0]["data"], "/wA=");
        assert_eq!(r["content"][0]["mimeType"], "image/png");
        assert!(r.get("structuredContent").is_none());
    }
}
